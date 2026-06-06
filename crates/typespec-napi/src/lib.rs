use napi_derive::napi;

#[napi]
pub fn hello() -> String {
    "hello from rust".to_string()
}
