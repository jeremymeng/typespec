use napi_derive::napi;

pub mod binder;
pub mod checker;
pub mod decorator;
pub mod parser;
pub mod program;
pub mod scanner;

#[napi]
pub fn hello() -> String {
    "hello from rust".to_string()
}
