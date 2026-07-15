use napi::bindgen_prelude::{Error, Result};

/// Error returned when an item handle is used after its item was deleted from
/// the module.
///
/// walrus' arena `get`/`get_mut` panic on a deleted id, and a panic across the
/// FFI boundary aborts the process. Item wrappers guard their arena access and
/// surface this catchable JS error instead.
pub(crate) fn deleted(kind: &str) -> Error {
  Error::from_reason(format!("this {kind} has been deleted from the module"))
}

/// Validate that a `ConstExpr` about to be stored in `module` only references
/// arena ids that are LIVE in that module.
///
/// walrus panics HARD at emit time (`get_global_index`/`get_func_index`) if a
/// stored `ConstExpr` references a global/function id that has no index in the
/// module — a foreign-module handle or an already-deleted id. That panic
/// crosses the FFI boundary and ABORTS the whole Node process, uncatchable.
/// Call this at every CONSUME site (e.g. `globals.addLocal(init)`) so a bad id
/// is rejected with a catchable JS error before it can reach emit.
///
/// This catches foreign + already-deleted ids at wiring time. It does NOT
/// prevent a LATER deletion of a still-referenced item from aborting emit —
/// walrus `delete` does not cascade (a known limitation); not solved here.
pub(crate) fn validate_const_expr(module: &walrus::Module, ce: &walrus::ConstExpr) -> Result<()> {
  use walrus::ConstExpr::*;
  match ce {
    Value(_) | RefNull(_) => Ok(()),
    Global(gid) => {
      if module.globals.iter().any(|g| g.id() == *gid) {
        Ok(())
      } else {
        Err(Error::from_reason(
          "ConstExpr references a global that is not in this module (or was deleted)",
        ))
      }
    }
    RefFunc(fid) => {
      if module.funcs.iter().any(|f| f.id() == *fid) {
        Ok(())
      } else {
        Err(Error::from_reason(
          "ConstExpr references a function that is not in this module (or was deleted)",
        ))
      }
    }
    // Our factories never build extended const expressions; reject rather than
    // risk an unvalidated embedded id reaching emit.
    Extended(_) => Err(Error::from_reason(
      "extended const expressions are not supported",
    )),
  }
}
