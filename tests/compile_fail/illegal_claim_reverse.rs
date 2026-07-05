use batpak::typestate::Transition;
use texo::events::machines::{supersede_claim, Current, Superseded};
use texo::events::payloads::ClaimSupersededV2;

fn reverse(payload: ClaimSupersededV2) -> Transition<Superseded, Current, ClaimSupersededV2> {
    supersede_claim(payload)
}

fn main() {}
