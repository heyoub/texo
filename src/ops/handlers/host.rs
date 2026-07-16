use super::common::{parse_input, run_op};
use crate::ops::env;
use serde::Deserialize;
use syncbat::HandlerResult;

#[syncbat::operation(
    descriptor = HOST_FINGERPRINT,
    register = register_host_fingerprint,
    register_item = host_fingerprint_item,
    name = "texo.host.fingerprint",
    effect = Inspect,
    input_schema = "texo.host.fingerprint.input.v2",
    output_schema = "texo.host.fingerprint.output.v2",
    receipt_kind = "receipt.texo.host.fingerprint.v2"
)]
#[tracing::instrument(skip_all)]
fn host_fingerprint(input: &[u8], _cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.host.fingerprint", || {
        let _input: HostFingerprintInput = parse_input("texo.host.fingerprint", input)?;
        env::with(|op_env| op_env.host_interface.clone())
    })
}
#[derive(Debug, Deserialize)]
struct HostFingerprintInput {}
