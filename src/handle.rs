use napi::bindgen_prelude::Error;

/// Error returned when an item handle is used after its item was deleted from
/// the module.
///
/// walrus' arena `get`/`get_mut` panic on a deleted id, and a panic across the
/// FFI boundary aborts the process. Item wrappers guard their arena access and
/// surface this catchable JS error instead.
pub(crate) fn deleted(kind: &str) -> Error {
  Error::from_reason(format!("this {kind} has been deleted from the module"))
}
