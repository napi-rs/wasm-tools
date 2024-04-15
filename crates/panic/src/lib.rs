use napi_derive::napi;

#[napi]
pub fn panic() {
  inner_call();
}

fn inner_call() {
  assert!(false, "assertion failed");
}
