use napi_derive::napi;

pub mod scanner;

#[napi]
pub fn hello() -> String {
    "hello from rust".to_string()
}
