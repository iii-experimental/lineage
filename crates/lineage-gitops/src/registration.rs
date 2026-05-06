use crate::ops;
use crate::{
    AttachTrailersInput, AttachTrailersOutput, ListShadowRefsInput, ResolveBlobInput,
    ResolveBlobOutput, RewindInput, ShadowRefEntry, ShadowSnapshotInput, ShadowSnapshotOutput,
};
use iii_sdk::{III, RegisterFunction};

pub fn register(iii: &III) {
    iii.register_function(RegisterFunction::new(
        "lineage::shadow_snapshot",
        |i: ShadowSnapshotInput| -> Result<ShadowSnapshotOutput, String> {
            ops::shadow_snapshot(i).map_err(|e| e.to_string())
        },
    ));

    iii.register_function(RegisterFunction::new(
        "lineage::rewind_to",
        |i: RewindInput| -> Result<(), String> { ops::rewind_to(i).map_err(|e| e.to_string()) },
    ));

    iii.register_function(RegisterFunction::new(
        "lineage::attach_trailers",
        |i: AttachTrailersInput| -> Result<AttachTrailersOutput, String> {
            ops::attach_trailers(i).map_err(|e| e.to_string())
        },
    ));

    iii.register_function(RegisterFunction::new(
        "lineage::list_shadow_refs",
        |i: ListShadowRefsInput| -> Result<Vec<ShadowRefEntry>, String> {
            ops::list_shadow_refs(i).map_err(|e| e.to_string())
        },
    ));

    iii.register_function(RegisterFunction::new(
        "lineage::resolve_blob",
        |i: ResolveBlobInput| -> Result<ResolveBlobOutput, String> {
            ops::resolve_blob(i).map_err(|e| e.to_string())
        },
    ));
}
