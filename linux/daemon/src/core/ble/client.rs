//! BLE GATT client per spec §6.1 and §9.1.
//!
//! L3 acts as the BLE Initiator: it scans, connects, discovers the Vortex
//! service, and reads/writes characteristics on Android's GATT server.

use std::time::Duration;

use bluer::gatt::remote::{Characteristic, CharacteristicWriteRequest, Service};
use bluer::gatt::WriteOp;
use bluer::{Adapter, Address, Result as BluerResult};
use futures::future::try_join_all;
use futures::{pin_mut, StreamExt};
use tokio::time::timeout;
use tracing::{debug, info};

use super::frame::{Frame, FrameDecodeError};
use super::{
    AdvDecodeError, AUDIO_SIGNAL_UUID, CAPABILITY_UUID, PAIRING_CONTROL_UUID,
    RECONNECT_CONTROL_UUID, VORTEX_SERVICE_UUID,
};

/// V1 Capability characteristic response (spec §9.1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityResponse {
    pub version: u8,
    pub capability_bits: u16,
}

#[derive(Debug)]
pub enum ClientError {
    Bluer(bluer::Error),
    Timeout(&'static str),
    NoVortexService,
    NoCharacteristic(&'static str),
    BadCapabilityResponse {
        len: usize,
    },
    UnsupportedVersion(u8),
    InvalidPayload(AdvDecodeError),
    FrameDecode(FrameDecodeError),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bluer(e) => write!(f, "bluer: {e}"),
            Self::Timeout(what) => write!(f, "timeout: {what}"),
            Self::NoVortexService => write!(f, "Vortex Service UUID not found on peer"),
            Self::NoCharacteristic(name) => write!(f, "{name} characteristic not found"),
            Self::BadCapabilityResponse { len } => {
                write!(f, "Capability response wrong length: {len} (expected 3)")
            }
            Self::UnsupportedVersion(v) => write!(f, "unsupported V1 version byte: {v:#04x}"),
            Self::InvalidPayload(e) => write!(f, "invalid advertisement payload: {e:?}"),
            Self::FrameDecode(e) => write!(f, "frame decode: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<bluer::Error> for ClientError {
    fn from(e: bluer::Error) -> Self {
        Self::Bluer(e)
    }
}

/// Connection handle to a peer's Vortex GATT service.
pub struct VortexClient {
    pub address: Address,
    pub service: Service,
    pub capability: Characteristic,
    pub pairing_control: Characteristic,
    pub reconnect_control: Characteristic,
    /// Audio-signal characteristic (Phase 2). `None` for older peers
    /// (A2 / A3 builds before P2.13) that didn't ship the characteristic
    /// yet — those still work, they just don't get the BLE fast-path
    /// and fall back to the 5-s LAN heartbeat.
    pub audio_signal: Option<Characteristic>,
}

impl VortexClient {
    /// Connect to the device at `address`, discover the Vortex Service,
    /// and resolve the four V1 characteristics.
    pub async fn connect(adapter: &Adapter, address: Address) -> Result<Self, ClientError> {
        let device = adapter.device(address)?;
        let t0 = std::time::Instant::now();
        if !device.is_connected().await.unwrap_or(false) {
            info!(%address, "GATT connect");
            timeout(Duration::from_secs(15), device.connect())
                .await
                .map_err(|_| ClientError::Timeout("connect"))?
                .map_err(ClientError::Bluer)?;
        }
        let connect_ms = t0.elapsed().as_millis();

        // Wait briefly for service discovery to populate. We resolve each
        // service's UUID asynchronously (via D-Bus) before checking the
        // match — see `find_vortex_service` for why we cannot block_on
        // from within an already-async context.
        let t1 = std::time::Instant::now();
        let service: Service = timeout(Duration::from_secs(15), async {
            loop {
                let svcs = device.services().await?;
                if let Some(s) = find_vortex_service(svcs).await? {
                    return Ok::<Service, bluer::Error>(s);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await
        .map_err(|_| ClientError::Timeout("service discovery"))?
        .map_err(ClientError::Bluer)?;
        info!(
            %address,
            connect_ms,
            discovery_ms = t1.elapsed().as_millis(),
            "GATT connect+discovery timing"
        );
        let chars = service.characteristics().await?;

        let capability = find_char(&chars, CAPABILITY_UUID, "Capability").await?;
        let pairing_control = find_char(&chars, PAIRING_CONTROL_UUID, "PairingControl").await?;
        let reconnect_control = find_char(&chars, RECONNECT_CONTROL_UUID, "ReconnectControl").await?;
        // Optional — A2/A3 builds before P2.13 didn't expose this.
        let audio_signal = find_char_opt(&chars, AUDIO_SIGNAL_UUID).await;

        Ok(Self {
            address,
            service,
            capability,
            pairing_control,
            reconnect_control,
            audio_signal,
        })
    }

    pub async fn read_capability(&self) -> Result<CapabilityResponse, ClientError> {
        let bytes = self.capability.read().await?;
        debug!(len = bytes.len(), "capability bytes");
        if bytes.len() < 3 {
            return Err(ClientError::BadCapabilityResponse { len: bytes.len() });
        }
        let version = bytes[0];
        if version != 0x01 {
            return Err(ClientError::UnsupportedVersion(version));
        }
        let capability_bits = u16::from_be_bytes([bytes[1], bytes[2]]);
        Ok(CapabilityResponse {
            version,
            capability_bits,
        })
    }

    /// Write a frame to the Pairing Control characteristic.
    ///
    /// Uses write-without-response per spec §9.1: pairing flow is
    /// driven by notify-on-write, the ATT-level ACK adds latency
    /// without any reliability benefit since we already gate on the
    /// notification echo.
    pub async fn write_pairing_control(&self, frame: &Frame) -> Result<(), ClientError> {
        let bytes = frame.encode();
        let req = CharacteristicWriteRequest {
            offset: 0,
            op_type: WriteOp::Command,
            prepare_authorize: false,
            ..Default::default()
        };
        self.pairing_control.write_ext(&bytes, &req).await?;
        Ok(())
    }

    /// Write a frame to the Reconnect Control characteristic.
    ///
    /// Same write-without-response rationale as `write_pairing_control`.
    pub async fn write_reconnect_control(&self, frame: &Frame) -> Result<(), ClientError> {
        let bytes = frame.encode();
        let req = CharacteristicWriteRequest {
            offset: 0,
            op_type: WriteOp::Command,
            prepare_authorize: false,
            ..Default::default()
        };
        self.reconnect_control.write_ext(&bytes, &req).await?;
        Ok(())
    }

    /// Send an echo request and wait for the matching echo response on
    /// Pairing Control notifications. Round-trip is bounded by `wait`.
    pub async fn echo_round_trip(
        &self,
        payload: Vec<u8>,
        wait: Duration,
    ) -> Result<Frame, ClientError> {
        let notifies = self.pairing_control.notify().await?;
        pin_mut!(notifies);
        let request = Frame::echo_request(payload);
        self.write_pairing_control(&request).await?;

        let bytes = timeout(wait, notifies.next())
            .await
            .map_err(|_| ClientError::Timeout("echo notify"))?
            .ok_or(ClientError::Timeout("notify stream closed"))?;
        let response = Frame::decode(&bytes).map_err(ClientError::FrameDecode)?;
        Ok(response)
    }

    pub async fn disconnect(self) -> BluerResult<()> {
        // The Service handle's parent device disconnect can be reached via
        // the adapter; bluer's high-level API does not expose a direct
        // disconnect on the Service. Callers can drop and let bluer GC.
        Ok(())
    }
}

async fn find_char(
    chars: &[Characteristic],
    uuid: uuid::Uuid,
    name: &'static str,
) -> Result<Characteristic, ClientError> {
    for c in chars {
        if c.uuid().await? == uuid {
            return Ok(c.clone());
        }
    }
    Err(ClientError::NoCharacteristic(name))
}

/// Same as `find_char` but returns `None` instead of an error when the
/// peer doesn't expose the UUID. Used for characteristics added in
/// later protocol revisions where missing is OK (older peer).
async fn find_char_opt(chars: &[Characteristic], uuid: uuid::Uuid) -> Option<Characteristic> {
    for c in chars {
        if c.uuid().await.ok()? == uuid {
            return Some(c.clone());
        }
    }
    None
}

/// Resolve every service's UUID concurrently and return the first one
/// whose UUID matches `VORTEX_SERVICE_UUID`. Replaces an older sync
/// helper that called `futures::executor::block_on` from inside the
/// tokio runtime — that pattern deadlocks bluer's D-Bus dispatcher
/// because the inner future yields back to the same executor.
async fn find_vortex_service(services: Vec<Service>) -> Result<Option<Service>, bluer::Error> {
    if services.is_empty() {
        return Ok(None);
    }
    let uuids = try_join_all(services.iter().map(|s| s.uuid())).await?;
    Ok(services
        .into_iter()
        .zip(uuids)
        .find(|(_, u)| *u == VORTEX_SERVICE_UUID)
        .map(|(s, _)| s))
}
