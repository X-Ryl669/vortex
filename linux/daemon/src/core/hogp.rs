//! Universal Control over **BLE HID** (HOGP) — no adb, no cable, no developer
//! mode.
//!
//! The Classic-Bluetooth HID path in `bt_hid.rs` is blocked on two changes we
//! will not make to someone's system: the adapter's Class of Device is
//! read-only on `Adapter1` (changing it means editing a root-owned config and
//! restarting bluetoothd, dropping every Bluetooth connection including the
//! user's earbuds), and BlueZ's `input` plugin would likely have to go, taking
//! Bluetooth mouse support away from everyone who has one.
//!
//! BLE sidesteps both: a peripheral says what it is through the Appearance
//! field of its own advertisement, set at runtime, and the Classic `input`
//! plugin is untouched.
//!
//! Proven on hardware before this module was written — see
//! `src/bin/hogp_test.rs`, which carries the result. The constants, the report
//! descriptor and the service layout below are lifted from that spike VERBATIM
//! rather than retyped: a HID report descriptor is exactly the kind of thing
//! that becomes subtly broken when transcribed, and those bytes are the ones a
//! real phone accepted.

use std::collections::BTreeSet;
use std::sync::Arc;

use bluer::adv::Advertisement;
use bluer::gatt::local::{
    Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
    CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, CharacteristicNotifier,
    Descriptor, DescriptorRead, Service,
};
use bluer::Uuid;
use futures::FutureExt;
use tokio::sync::Mutex;

/// Expand a 16-bit assigned number into the full Bluetooth base UUID.
fn uuid16(v: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb_u128 | ((v as u128) << 96))
}

// ---- Assigned numbers we need (Bluetooth SIG) ----
const SVC_DEVICE_INFO: u16 = 0x180a;
const SVC_BATTERY: u16 = 0x180f;
const SVC_HID: u16 = 0x1812;

const CHR_BATTERY_LEVEL: u16 = 0x2a19;
const CHR_PNP_ID: u16 = 0x2a50;
const CHR_HID_INFO: u16 = 0x2a4a;
const CHR_REPORT_MAP: u16 = 0x2a4b;
const CHR_HID_CONTROL_POINT: u16 = 0x2a4c;
const CHR_REPORT: u16 = 0x2a4d;
const CHR_PROTOCOL_MODE: u16 = 0x2a4e;
/// Report Reference — tells the host which report this characteristic carries.
const DSC_REPORT_REFERENCE: u16 = 0x2908;

/// Appearance: Human Interface Device / Mouse. This is the field that makes
/// Android offer the laptop as a pointing device — the BLE stand-in for the
/// Class of Device we cannot set on the Classic path.
const APPEARANCE_MOUSE: u16 = 0x03c2;

/// The advertised name. Keep it SHORT: a legacy advertisement carries 31 bytes
/// total, and flags (3) + the 16-bit service UUID (4) + appearance (4) already
/// spend 11. "Vortex Laptop Mouse" wanted 21 more — 32 in all — leaving BlueZ
/// no room for the name.
const ADV_NAME: &str = "Vortex Mouse";

/// Report ID 1, matching the descriptor below and the Report Reference.
const REPORT_ID_MOUSE: u8 = 0x01;

/// A plain 3-button relative mouse: buttons byte, then dx/dy/wheel as signed
/// bytes. Report layout on the wire is `[buttons, dx, dy, wheel]`.
const REPORT_MAP: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x02, // Usage (Mouse)
    0xa1, 0x01, // Collection (Application)
    0x85, REPORT_ID_MOUSE, //   Report ID (1)
    0x09, 0x01, //   Usage (Pointer)
    0xa1, 0x00, //   Collection (Physical)
    0x05, 0x09, //     Usage Page (Button)
    0x19, 0x01, //     Usage Minimum (Button 1)
    0x29, 0x03, //     Usage Maximum (Button 3)
    0x15, 0x00, //     Logical Minimum (0)
    0x25, 0x01, //     Logical Maximum (1)
    0x95, 0x03, //     Report Count (3)
    0x75, 0x01, //     Report Size (1)
    0x81, 0x02, //     Input (Data, Variable, Absolute)
    0x95, 0x01, //     Report Count (1)
    0x75, 0x05, //     Report Size (5)
    0x81, 0x03, //     Input (Constant) — padding to a whole byte
    0x05, 0x01, //     Usage Page (Generic Desktop)
    0x09, 0x30, //     Usage (X)
    0x09, 0x31, //     Usage (Y)
    0x09, 0x38, //     Usage (Wheel)
    0x15, 0x81, //     Logical Minimum (-127)
    0x25, 0x7f, //     Logical Maximum (127)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x03, //     Report Count (3)
    0x81, 0x06, //     Input (Data, Variable, Relative)
    0xc0, //   End Collection
    0xc0, // End Collection
];

fn const_read(uuid: u16, value: Vec<u8>) -> Characteristic {
    Characteristic {
        uuid: uuid16(uuid),
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: Box::new(move |_req| {
                let value = value.clone();
                async move { Ok(value) }.boxed()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}


/// A live HOGP peripheral: advertises as a mouse and pushes input reports to
/// whichever host has subscribed.
#[derive(Clone)]
pub struct HogpServer {
    /// `Some` once the phone has subscribed to the Report characteristic. That
    /// subscription — not the bond, and not the connection — is the thing that
    /// means reports will actually be delivered.
    notifier: Arc<Mutex<Option<CharacteristicNotifier>>>,
    /// Held for as long as the server should exist: dropping either handle
    /// withdraws the service and the advertisement, which is how `stop()`
    /// leaves nothing behind.
    _handles: Arc<Mutex<Option<(bluer::gatt::local::ApplicationHandle, bluer::adv::AdvertisementHandle)>>>,
}

impl HogpServer {
    /// Publish the HID service and start advertising. The phone bonds once,
    /// from its own Bluetooth settings; after that it reconnects on its own
    /// whenever this advertisement is up.
    pub async fn start(adapter: &bluer::Adapter) -> bluer::Result<Self> {
        let notifier: Arc<Mutex<Option<CharacteristicNotifier>>> = Arc::new(Mutex::new(None));
        let hid_service = build_hid_service(notifier.clone());
        // HOGP expects Device Information and Battery alongside HID. Android
        // reads the PnP ID to name and classify the device, and a HID service
        // arriving without these is routinely ignored.
        let app = Application {
            services: vec![
                Service {
                    uuid: uuid16(SVC_DEVICE_INFO),
                    primary: true,
                    characteristics: vec![const_read(
                        CHR_PNP_ID,
                        vec![0x02, 0x6b, 0x1d, 0x46, 0x02, 0x00, 0x01],
                    )],
                    ..Default::default()
                },
                Service {
                    uuid: uuid16(SVC_BATTERY),
                    primary: true,
                    characteristics: vec![const_read(CHR_BATTERY_LEVEL, vec![100])],
                    ..Default::default()
                },
                hid_service,
            ],
            ..Default::default()
        };
        let app_handle = adapter.serve_gatt_application(app).await?;
        let adv = Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids: BTreeSet::from([uuid16(SVC_HID)]),
            discoverable: Some(true),
            local_name: Some(ADV_NAME.to_string()),
            appearance: Some(APPEARANCE_MOUSE),
            ..Default::default()
        };
        let adv_handle = adapter.advertise(adv).await?;
        tracing::info!("hogp: advertising as '{ADV_NAME}' (appearance 0x{APPEARANCE_MOUSE:04x})");
        Ok(Self {
            notifier,
            _handles: Arc::new(Mutex::new(Some((app_handle, adv_handle)))),
        })
    }

    /// Has a host subscribed? Only then does sending a report mean anything —
    /// which is why this, and not "is something connected", is what the
    /// transport gate should ask.
    pub async fn is_ready(&self) -> bool {
        self.notifier.lock().await.is_some()
    }

    /// Withdraw the service and the advertisement. Nothing is left configured
    /// on this machine; the phone keeps its bond and will simply find nothing
    /// to reconnect to until the next `start`.
    pub async fn stop(&self) {
        *self._handles.lock().await = None;
        *self.notifier.lock().await = None;
        tracing::info!("hogp: stopped advertising");
    }

    /// Relative pointer movement and wheel, as the descriptor lays them out:
    /// `[buttons, dx, dy, wheel]`. No `0xa1` transaction header — that belongs
    /// to Classic HID over L2CAP, not to a GATT notification.
    pub async fn send_report(&self, buttons: u8, dx: i8, dy: i8, wheel: i8) -> bool {
        let mut guard = self.notifier.lock().await;
        let Some(n) = guard.as_mut() else { return false };
        match n.notify(vec![buttons, dx as u8, dy as u8, wheel as u8]).await {
            Ok(()) => true,
            Err(e) => {
                // The host went away. Drop the notifier so `is_ready` reports
                // the truth and the caller can fall back rather than spending
                // a session writing into nothing.
                tracing::info!("hogp: notify failed ({e}); waiting for a new subscription");
                *guard = None;
                false
            }
        }
    }
}

fn build_hid_service(notifier: Arc<Mutex<Option<CharacteristicNotifier>>>) -> Service {
    Service {
        uuid: uuid16(SVC_HID),
        primary: true,
        characteristics: vec![
            // bcdHID 0x0111, country code 0 (not localised), flags 0x03 =
            // remote-wake + normally-connectable.
            const_read(CHR_HID_INFO, vec![0x11, 0x01, 0x00, 0x03]),
            const_read(CHR_REPORT_MAP, REPORT_MAP.to_vec()),
            // Report protocol (1), not boot protocol (0). Writable because the
            // host is allowed to switch us, but we only ever report in report
            // protocol, so the write is accepted and ignored.
            Characteristic {
                uuid: uuid16(CHR_PROTOCOL_MODE),
                read: Some(CharacteristicRead {
                    read: true,
                    encrypt_read: true,
                    fun: Box::new(|_| async move { Ok(vec![0x01]) }.boxed()),
                    ..Default::default()
                }),
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Fun(Box::new(|v, _| {
                        async move {
                            tracing::debug!("hogp: host set protocol mode -> {v:?}");
                            Ok(())
                        }
                        .boxed()
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // Suspend / exit-suspend from the host. Nothing to do, but the
            // characteristic has to exist or the host rejects the service.
            Characteristic {
                uuid: uuid16(CHR_HID_CONTROL_POINT),
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Fun(Box::new(|v, _| {
                        async move {
                            tracing::debug!("hogp: host wrote control point -> {v:?}");
                            Ok(())
                        }
                        .boxed()
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // The input report itself: read for the host's initial fetch,
            // notify for everything after. The Report Reference descriptor is
            // what ties these bytes to report ID 1 as an *Input* report — omit
            // it and the host has no idea what it just subscribed to.
            Characteristic {
                uuid: uuid16(CHR_REPORT),
                read: Some(CharacteristicRead {
                    read: true,
                    encrypt_read: true,
                    fun: Box::new(|_| async move { Ok(vec![0, 0, 0, 0]) }.boxed()),
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Fun({
                        let notifier = notifier.clone();
                        Box::new(move |n| {
                            let notifier = notifier.clone();
                            async move {
                                // The moment that matters: reports are deliverable from here.
                                tracing::info!("hogp: host subscribed — cursor can cross with no adb");
                                *notifier.lock().await = Some(n);
                            }
                            .boxed()
                        })
                    }),
                    ..Default::default()
                }),
                descriptors: vec![Descriptor {
                    uuid: uuid16(DSC_REPORT_REFERENCE),
                    read: Some(DescriptorRead {
                        read: true,
                        encrypt_read: true,
                        // [report ID, type] where type 1 = Input.
                        fun: Box::new(|_| async move { Ok(vec![REPORT_ID_MOUSE, 0x01]) }.boxed()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
