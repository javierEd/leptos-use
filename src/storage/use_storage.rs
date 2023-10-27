use crate::{
    core::{MaybeRwSignal, StorageType},
    use_event_listener_with_options, use_window, UseEventListenerOptions,
};
use cfg_if::cfg_if;
use leptos::*;
use std::{rc::Rc, str::FromStr};
use thiserror::Error;
use wasm_bindgen::JsValue;

#[derive(Clone)]
pub struct UseStorageOptions<T: 'static, C: Codec<T>> {
    codec: C,
    on_error: Rc<dyn Fn(UseStorageError<C::Error>)>,
    listen_to_storage_changes: bool,
    default_value: MaybeSignal<T>,
}

/// Session handling errors returned by [`use_storage`].
#[derive(Error, Debug)]
pub enum UseStorageError<Err> {
    #[error("storage not available")]
    StorageNotAvailable(JsValue),
    #[error("storage not returned from window")]
    StorageReturnedNone,
    #[error("failed to get item")]
    GetItemFailed(JsValue),
    #[error("failed to set item")]
    SetItemFailed(JsValue),
    #[error("failed to delete item")]
    RemoveItemFailed(JsValue),
    #[error("failed to encode / decode item value")]
    ItemCodecError(Err),
}

/// Hook for using local storage. Returns a result of a signal and a setter / deleter.
pub fn use_local_storage<T>(key: impl AsRef<str>) -> (Memo<T>, impl Fn(Option<T>) -> ())
where
    T: Clone + Default + FromStr + PartialEq + ToString,
{
    use_storage_with_options(StorageType::Local, key, UseStorageOptions::string_codec())
}

pub fn use_local_storage_with_options<T, C>(
    key: impl AsRef<str>,
    options: UseStorageOptions<T, C>,
) -> (Memo<T>, impl Fn(Option<T>) -> ())
where
    T: Clone + PartialEq,
    C: Codec<T>,
{
    use_storage_with_options(StorageType::Local, key, options)
}

/// Hook for using session storage. Returns a result of a signal and a setter / deleter.
pub fn use_session_storage<T>(key: impl AsRef<str>) -> (Memo<T>, impl Fn(Option<T>) -> ())
where
    T: Clone + Default + FromStr + PartialEq + ToString,
{
    use_storage_with_options(StorageType::Session, key, UseStorageOptions::string_codec())
}

pub fn use_session_storage_with_options<T, C>(
    key: impl AsRef<str>,
    options: UseStorageOptions<T, C>,
) -> (Memo<T>, impl Fn(Option<T>) -> ())
where
    T: Clone + PartialEq,
    C: Codec<T>,
{
    use_storage_with_options(StorageType::Session, key, options)
}

/// Hook for using any kind of storage. Returns a result of a signal and a setter / deleter.
pub fn use_storage_with_options<T, C>(
    storage_type: StorageType,
    key: impl AsRef<str>,
    options: UseStorageOptions<T, C>,
) -> (Memo<T>, impl Fn(Option<T>) -> ())
where
    T: Clone + PartialEq,
    C: Codec<T>,
{
    cfg_if! { if #[cfg(feature = "ssr")] {
        let (data, set_data) = create_signal(None);
        let set_value = move |value: Option<T>| {
            set_data.set(value);
        };
        let value = create_memo(move |_| data.get().unwrap_or_default());
        return (value, set_value);
    } else {
        // Continue
    }}

    let UseStorageOptions {
        codec,
        on_error,
        listen_to_storage_changes,
        default_value,
    } = options;

    // Get storage API
    let storage = storage_type
        .into_storage()
        .map_err(UseStorageError::StorageNotAvailable)
        .and_then(|s| s.ok_or(UseStorageError::StorageReturnedNone));
    let storage = handle_error(&on_error, storage);

    // Fetch initial value (undecoded)
    let initial_value = storage
        .to_owned()
        // Pull from storage
        .and_then(|s| {
            let result = s
                .get_item(key.as_ref())
                .map_err(UseStorageError::GetItemFailed);
            handle_error(&on_error, result)
        })
        .unwrap_or_default();
    // Decode initial value
    let initial_value = decode_item(&codec, initial_value, &on_error);

    let (data, set_data) = create_signal(initial_value);

    // Update storage value
    let set_value = {
        let storage = storage.to_owned();
        let key = key.as_ref().to_owned();
        let codec = codec.to_owned();
        let on_error = on_error.to_owned();
        move |value: Option<T>| {
            let key = key.as_str();
            // Attempt to update storage
            let _ = storage.as_ref().map(|storage| {
                let result = match value {
                    // Update
                    Some(ref value) => codec
                        .encode(&value)
                        .map_err(UseStorageError::ItemCodecError)
                        .and_then(|enc_value| {
                            storage
                                .set_item(key, &enc_value)
                                .map_err(UseStorageError::SetItemFailed)
                        }),
                    // Remove
                    None => storage
                        .remove_item(key)
                        .map_err(UseStorageError::RemoveItemFailed),
                };
                handle_error(&on_error, result)
            });

            // Notify signal of change
            set_data.set(value);
        }
    };

    // Listen for storage events
    // Note: we only receive events from other tabs / windows, not from internal updates.
    if listen_to_storage_changes {
        let key = key.as_ref().to_owned();
        let _ = use_event_listener_with_options(
            use_window(),
            leptos::ev::storage,
            move |ev| {
                // Update storage value if our key matches
                if let Some(k) = ev.key() {
                    if k == key {
                        let value = decode_item(&codec, ev.new_value(), &on_error);
                        set_data.set(value)
                    }
                } else {
                    // All keys deleted
                    set_data.set(None)
                }
            },
            UseEventListenerOptions::default().passive(true),
        );
    };

    // Apply default value
    let value = create_memo(move |_| data.get().unwrap_or_else(|| default_value.get()));

    (value, set_value)
}

/// Calls the on_error callback with the given error. Removes the error from the Result to avoid double error handling.
fn handle_error<T, Err>(
    on_error: &Rc<dyn Fn(UseStorageError<Err>)>,
    result: Result<T, UseStorageError<Err>>,
) -> Result<T, ()> {
    result.or_else(|err| Err((on_error)(err)))
}

fn decode_item<T, C: Codec<T>>(
    codec: &C,
    str: Option<String>,
    on_error: &Rc<dyn Fn(UseStorageError<C::Error>)>,
) -> Option<T> {
    str.map(|str| {
        let result = codec.decode(str).map_err(UseStorageError::ItemCodecError);
        handle_error(&on_error, result)
    })
    .transpose()
    // We've sent our error so unwrap to drop () error
    .unwrap_or_default()
}

impl<T: Clone + Default, C: Codec<T>> UseStorageOptions<T, C> {
    pub(super) fn new(codec: C) -> Self {
        Self {
            codec,
            on_error: Rc::new(|_err| ()),
            listen_to_storage_changes: true,
            default_value: MaybeSignal::default(),
        }
    }

    pub fn on_error(self, on_error: impl Fn(UseStorageError<C::Error>) + 'static) -> Self {
        Self {
            on_error: Rc::new(on_error),
            ..self
        }
    }

    pub fn listen_to_storage_changes(self, listen_to_storage_changes: bool) -> Self {
        Self {
            listen_to_storage_changes,
            ..self
        }
    }

    pub fn default_value(self, values: impl Into<MaybeRwSignal<T>>) -> Self {
        Self {
            default_value: values.into().into_signal().0.into(),
            ..self
        }
    }
}

pub trait Codec<T>: Clone + 'static {
    type Error;
    fn encode(&self, val: &T) -> Result<String, Self::Error>;
    fn decode(&self, str: String) -> Result<T, Self::Error>;
}

#[derive(Clone, PartialEq)]
pub struct StringCodec();

impl<T: FromStr + ToString> Codec<T> for StringCodec {
    type Error = T::Err;

    fn encode(&self, val: &T) -> Result<String, Self::Error> {
        Ok(val.to_string())
    }

    fn decode(&self, str: String) -> Result<T, Self::Error> {
        T::from_str(&str)
    }
}

impl<T: Clone + Default + FromStr + ToString> UseStorageOptions<T, StringCodec> {
    pub fn string_codec() -> Self {
        Self::new(StringCodec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_codec() {
        let s = String::from("party time 🎉");
        let codec = StringCodec();
        assert_eq!(codec.encode(&s), Ok(s.clone()));
        assert_eq!(codec.decode(s.clone()), Ok(s));
    }
}
