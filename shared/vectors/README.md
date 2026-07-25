# Shared A2/L2 Test Vectors

This directory will hold deterministic JSON vectors used by both Android A2 and
Linux L2.

Required vector groups are defined in the V1 test-vectors spec.

Initial groups:

- `identity`
- `noise_xx_first_pairing`
- `sas_v1`
- `session_export_secret_v1`
- `session_id_v1`
- `pairwise_reconnect_secret_v1`
- `presence_token_v1`
- `noise_ik_reconnect`
- `channel_join_v1`
- `envelope_replay_v1`
- `fragment_reassembly_v1`

Placeholders are allowed during planning only. Crypto-dependent implementation
must not be accepted until Android and Linux pass the same vectors.
