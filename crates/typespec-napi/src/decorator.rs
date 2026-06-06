use std::collections::HashSet;

use napi::bindgen_prelude::{External, Function, JsObjectValue, Object, ToNapiValue, Unknown};
use napi::{sys, Env, Error, JsValue, Result, Status, ValueType};
use typespec_checker::{
    DecoratorApplication, DecoratorArgument, DecoratorContext, DecoratorRunner, DecoratorValue,
    TypeId,
};

#[derive(Clone, Copy)]
pub struct TypeHandle {
    pub type_id: TypeId,
}

pub struct NapiDecoratorRunner<'env> {
    env: &'env Env,
    registry: Option<Object<'env>>,
    names: HashSet<String>,
}

impl<'env> NapiDecoratorRunner<'env> {
    pub fn new(env: &'env Env, registry: Option<Object<'env>>) -> Result<Self> {
        let names = if let Some(registry) = &registry {
            Object::keys(registry)?.into_iter().collect()
        } else {
            HashSet::new()
        };
        Ok(Self {
            env,
            registry,
            names,
        })
    }

    fn resolve_name<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        if self.names.contains(name) {
            Some(name)
        } else if name.starts_with('@') {
            let qualified = format!("TypeSpec.{name}");
            self.names.get(&qualified).map(String::as_str)
        } else {
            None
        }
    }

    fn context_object(
        &self,
        checker: *mut dyn DecoratorContext,
        active_decorator: String,
        decorator_node: typespec_parser::ast::NodeId,
    ) -> Result<Object<'env>> {
        let mut context = Object::new(self.env)?;
        context.set_named_property("program", Object::new(self.env)?)?;

        let get_property_context = CallbackContext {
            checker,
            active_decorator: active_decorator.clone(),
            decorator_node,
        };
        context.set_named_property(
            "getProperty",
            self.env
                .create_function_from_closure::<Unknown, Unknown, _>(
                    "getProperty",
                    move |call| -> Result<Unknown<'static>> {
                        let target: &External<TypeHandle> = call.get(0)?;
                        let key: String = call.get(1)?;
                        let value = with_context(get_property_context.checker, |context| {
                            context.get_decorator_property(
                                &get_property_context.active_decorator,
                                target.type_id,
                                &key,
                            )
                        });
                        value_to_unknown(call.env, value)
                    },
                )?,
        )?;

        let set_property_context = CallbackContext {
            checker,
            active_decorator: active_decorator.clone(),
            decorator_node,
        };
        context.set_named_property(
            "setProperty",
            self.env.create_function_from_closure::<Unknown, (), _>(
                "setProperty",
                move |call| -> Result<()> {
                    let target: &External<TypeHandle> = call.get(0)?;
                    let key: String = call.get(1)?;
                    let value: Unknown = call.get(2)?;
                    let value = unknown_to_value(value)?;
                    with_context(set_property_context.checker, |context| {
                        context.set_decorator_property(
                            &set_property_context.active_decorator,
                            target.type_id,
                            &key,
                            value,
                        );
                    });
                    Ok(())
                },
            )?,
        )?;

        let get_name_context = CallbackContext {
            checker,
            active_decorator: active_decorator.clone(),
            decorator_node,
        };
        context.set_named_property(
            "getName",
            self.env
                .create_function_from_closure::<Unknown, String, _>(
                    "getName",
                    move |call| -> Result<String> {
                        let target: &External<TypeHandle> = call.get(0)?;
                        Ok(with_context(get_name_context.checker, |context| {
                            context.get_type_name(target.type_id)
                        }))
                    },
                )?,
        )?;

        let get_kind_context = CallbackContext {
            checker,
            active_decorator: active_decorator.clone(),
            decorator_node,
        };
        context.set_named_property(
            "getKind",
            self.env
                .create_function_from_closure::<Unknown, String, _>(
                    "getKind",
                    move |call| -> Result<String> {
                        let target: &External<TypeHandle> = call.get(0)?;
                        Ok(with_context(get_kind_context.checker, |context| {
                            context.get_type_kind(target.type_id)
                        }))
                    },
                )?,
        )?;

        let report_context = CallbackContext {
            checker,
            active_decorator,
            decorator_node,
        };
        context.set_named_property(
            "reportDiagnostic",
            self.env.create_function_from_closure::<Unknown, (), _>(
                "reportDiagnostic",
                move |call| -> Result<()> {
                    let diagnostic: Object = call.get(0)?;
                    let code: String = diagnostic
                        .get_named_property("code")
                        .unwrap_or_else(|_| "decorator-diagnostic".to_string());
                    let message: String = diagnostic
                        .get_named_property("message")
                        .unwrap_or_else(|_| code.clone());
                    let target = if diagnostic.has_named_property("target")? {
                        let target: &External<TypeHandle> =
                            diagnostic.get_named_property("target")?;
                        Some(target.type_id)
                    } else {
                        None
                    };
                    with_context(report_context.checker, |context| {
                        context.report_decorator_diagnostic(
                            code,
                            message,
                            target,
                            report_context.decorator_node,
                        );
                    });
                    Ok(())
                },
            )?,
        )?;

        Ok(context)
    }

    fn unknown_for_argument(&self, arg: &DecoratorArgument) -> Result<Unknown<'static>> {
        match arg {
            DecoratorArgument::String(value) => to_unknown(self.env, value.clone()),
            DecoratorArgument::Number(value) => to_unknown(self.env, *value),
            DecoratorArgument::Boolean(value) => to_unknown(self.env, *value),
            DecoratorArgument::TypeRef(type_id) => type_handle_to_unknown(self.env, *type_id),
            DecoratorArgument::None => value_to_unknown(self.env, None),
        }
    }
}

impl DecoratorRunner for NapiDecoratorRunner<'_> {
    fn is_enabled(&self) -> bool {
        self.registry.is_some()
    }

    fn call_decorator(
        &mut self,
        checker: &mut dyn DecoratorContext,
        target: TypeId,
        application: &DecoratorApplication,
    ) {
        let Some(registry_name) = self.resolve_name(&application.name) else {
            checker.report_decorator_diagnostic(
                "unknown-decorator".to_string(),
                format!("Unknown decorator '{}'.", application.name),
                Some(target),
                application.node,
            );
            return;
        };
        let Some(registry) = &self.registry else {
            return;
        };
        let function = match registry.get_named_property::<Function>(registry_name) {
            Ok(function) => function,
            Err(error) => {
                checker.report_decorator_diagnostic(
                    "unknown-decorator".to_string(),
                    format!("Unable to load decorator '{}': {}", application.name, error),
                    Some(target),
                    application.node,
                );
                return;
            }
        };

        let checker_ptr = checker as *mut dyn DecoratorContext;
        // SAFETY: napi stores callbacks as `'static`, but this pointer is only
        // dereferenced while the JS decorator is called synchronously below.
        // The context helpers become invalid when that call returns.
        let checker_ptr: *mut (dyn DecoratorContext + 'static) =
            unsafe { std::mem::transmute(checker_ptr) };
        let context =
            match self.context_object(checker_ptr, registry_name.to_string(), application.node) {
                Ok(context) => context,
                Err(error) => {
                    checker.report_decorator_diagnostic(
                        "decorator-exception".to_string(),
                        format!(
                            "Unable to create decorator context '{}': {}",
                            application.name, error
                        ),
                        Some(target),
                        application.node,
                    );
                    return;
                }
            };
        // SAFETY: `context` was just allocated in this Env and is kept alive
        // for the duration of the synchronous decorator call below.
        let context = unsafe { Unknown::from_raw_unchecked(self.env.raw(), context.raw()) };
        let target = match type_handle_to_unknown(self.env, target) {
            Ok(target) => target,
            Err(error) => {
                checker.report_decorator_diagnostic(
                    "decorator-exception".to_string(),
                    format!(
                        "Unable to marshal decorator target '{}': {}",
                        application.name, error
                    ),
                    None,
                    application.node,
                );
                return;
            }
        };
        let args = match application
            .args
            .iter()
            .map(|arg| self.unknown_for_argument(arg))
            .collect::<Result<Vec<_>>>()
        {
            Ok(args) => args,
            Err(error) => {
                checker.report_decorator_diagnostic(
                    "decorator-exception".to_string(),
                    format!(
                        "Unable to marshal decorator arguments '{}': {}",
                        application.name, error
                    ),
                    None,
                    application.node,
                );
                return;
            }
        };

        let result = call_decorator_function(self.env, function, context, target, &args);
        if let Err(error) = result {
            checker.report_decorator_diagnostic(
                "decorator-exception".to_string(),
                format!("Decorator '{}' threw: {}", application.name, error),
                None,
                application.node,
            );
        }
    }
}

#[derive(Clone)]
struct CallbackContext {
    checker: *mut dyn DecoratorContext,
    active_decorator: String,
    decorator_node: typespec_parser::ast::NodeId,
}

fn with_context<R>(
    checker: *mut dyn DecoratorContext,
    f: impl FnOnce(&mut dyn DecoratorContext) -> R,
) -> R {
    // SAFETY: The pointer is created from the live checker immediately before a
    // synchronous JS decorator invocation on the same JS thread. The context
    // object is not valid after that call returns; decorator helpers must not be
    // retained for later asynchronous use.
    let checker = unsafe { &mut *checker };
    f(checker)
}

fn value_to_unknown(env: &Env, value: Option<DecoratorValue>) -> Result<Unknown<'static>> {
    match value {
        Some(DecoratorValue::String(value)) => to_unknown(env, value),
        Some(DecoratorValue::Number(value)) => to_unknown(env, value),
        Some(DecoratorValue::Boolean(value)) => to_unknown(env, value),
        Some(DecoratorValue::TypeRef(type_id)) => type_handle_to_unknown(env, type_id),
        Some(DecoratorValue::None) | None => {
            // SAFETY: converting Rust `None` to a JS null value in the active Env.
            let raw = unsafe { <Option<String> as ToNapiValue>::to_napi_value(env.raw(), None)? };
            // SAFETY: `raw` was produced by napi for this Env immediately above.
            Ok(unsafe { Unknown::from_raw_unchecked(env.raw(), raw) })
        }
    }
}

fn to_unknown<T: ToNapiValue>(env: &Env, value: T) -> Result<Unknown<'static>> {
    // SAFETY: `ToNapiValue` creates a value owned by the provided Env.
    let raw = unsafe { T::to_napi_value(env.raw(), value)? };
    // SAFETY: `raw` belongs to `env` and is returned immediately to JS.
    Ok(unsafe { Unknown::from_raw_unchecked(env.raw(), raw) })
}

fn type_handle_to_unknown(env: &Env, type_id: TypeId) -> Result<Unknown<'static>> {
    // SAFETY: the External contains a copyable TypeId handle, not a borrowed
    // pointer into the checker arena.
    let raw = unsafe {
        <External<TypeHandle> as ToNapiValue>::to_napi_value(
            env.raw(),
            External::new(TypeHandle { type_id }),
        )?
    };
    // SAFETY: `raw` belongs to `env` and wraps the External allocated above.
    Ok(unsafe { Unknown::from_raw_unchecked(env.raw(), raw) })
}

fn unknown_to_value(value: Unknown) -> Result<DecoratorValue> {
    match value.get_type()? {
        // SAFETY: the ValueType check immediately above guarantees the cast target.
        ValueType::String => Ok(DecoratorValue::String(unsafe { value.cast::<String>()? })),
        // SAFETY: the ValueType check immediately above guarantees the cast target.
        ValueType::Number => Ok(DecoratorValue::Number(unsafe { value.cast::<f64>()? })),
        // SAFETY: the ValueType check immediately above guarantees the cast target.
        ValueType::Boolean => Ok(DecoratorValue::Boolean(unsafe { value.cast::<bool>()? })),
        ValueType::External => {
            // SAFETY: M6 only passes TypeHandle Externals to decorator helpers.
            let handle = unsafe { value.cast::<&External<TypeHandle>>()? };
            Ok(DecoratorValue::TypeRef(handle.type_id))
        }
        ValueType::Null | ValueType::Undefined => Ok(DecoratorValue::None),
        _ => Ok(DecoratorValue::None),
    }
}

fn call_decorator_function(
    env: &Env,
    function: Function<'_>,
    context: Unknown<'static>,
    target: Unknown<'static>,
    decorator_args: &[Unknown<'static>],
) -> Result<Unknown<'static>> {
    let mut raw_this = std::ptr::null_mut();
    napi::check_status!(
        // SAFETY: `raw_this` is an out-parameter for the active Env.
        unsafe { sys::napi_get_undefined(env.raw(), &mut raw_this) },
        "Get undefined value failed"
    )?;
    let mut raw_args = Vec::with_capacity(decorator_args.len() + 2);
    // SAFETY: All values were created in the same Env immediately before this
    // synchronous call and remain live until the call returns.
    raw_args.push(context.raw());
    raw_args.push(target.raw());
    for arg in decorator_args {
        raw_args.push(arg.raw());
    }
    let mut raw_return = std::ptr::null_mut();
    // SAFETY: `function` and every argument belong to `env`; the call happens
    // synchronously on the JS thread that entered the native `check` binding.
    napi::check_pending_exception!(
        env.raw(),
        // SAFETY: `function` and every argument belong to `env`; the call
        // happens synchronously on the JS thread that entered `check`.
        unsafe {
            sys::napi_call_function(
                env.raw(),
                raw_this,
                function.raw(),
                raw_args.len(),
                raw_args.as_ptr(),
                &mut raw_return,
            )
        },
        "Call decorator failed"
    )?;
    if raw_return.is_null() {
        return Err(Error::new(
            Status::GenericFailure,
            "Decorator returned a null napi value".to_string(),
        ));
    }
    // SAFETY: napi returned `raw_return` for this Env from the call above.
    Ok(unsafe { Unknown::from_raw_unchecked(env.raw(), raw_return) })
}
