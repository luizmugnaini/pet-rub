use serde::{Deserialize, Serialize};
use strum::Display;

use crate::components::{ktree, lei, patchsets};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KtreeStatus {
    Success(String),
    Failed(String),
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
pub enum Action {
    Tick,
    Render,
    Resize(u16, u16),
    Suspend,
    Resume,
    Quit,
    ClearScreen,
    Error(String),
    Help,
    LeiSetMode(lei::LocalMode),
    LeiFetchPatchsets,
    PatchsetsList(String),
    PatchsetsAddIndex,
    PatchsetsSubIndex,
    PatchsetsThread,
    PatchsetsSetMode(patchsets::LocalMode),
    KtreeApply(String),
    KtreeSetMode(ktree::LocalMode),
    KtreeAbort,
    KtreeResult(KtreeStatus),
}
