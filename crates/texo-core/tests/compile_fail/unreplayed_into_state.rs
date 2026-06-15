//! Compile-fail: cannot call into_state on Unreplayed reducer.

use texo_core::replay::reducer::{ReplayReducer, Unreplayed};

fn main() {
    let _ = ReplayReducer::<Unreplayed>::new().into_state();
}
