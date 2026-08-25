use std::{
    any::{Any, TypeId, type_name},
    collections::HashMap,
    sync::LazyLock,
};

use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::watch;
use utoipa::openapi::RefOr;

use crate::AppError;

fn camel_to_snake_case(input: &str) -> String {
    let mut snake = String::new();
    for (i, ch) in input.char_indices() {
        if i > 0 && ch.is_uppercase() {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

// TODO: derive macro
pub trait ConfigValue:
    'static + Send + Sync + Default + Clone + Serialize + DeserializeOwned + utoipa::ToSchema
{
    const KEY: Option<&str> = None;
    const ENV_KEY: Option<&str> = None;
    const REQUIRE_RESTART: bool = false;
}

#[derive(Debug, Default)]
struct SettingValue<T> {
    default: T,
    config: Option<T>,
    cli: Option<T>,
    env: Option<T>,
}

#[derive(Debug, Serialize)]
pub struct SerializedSetting {
    require_restart: bool,
    key: String,
    default_value: serde_json::Value,
    config_value: serde_json::Value,
    cli_value: serde_json::Value,
    env_value: serde_json::Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConfigurationApplyError {
    pub message: String,
    pub key: String,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct ConfigurationApplyResult {
    pub require_restart: bool,
    pub errors: Vec<ConfigurationApplyError>,
}

impl<T: ConfigValue> SettingValue<T> {
    pub fn new(val: T) -> Self {
        use std::env::var;
        let env = match T::ENV_KEY {
            Some(key) => Some(key.to_string()),
            None => Some(T::KEY.map(str::to_uppercase).unwrap_or_else(|| {
                let name = T::name();
                camel_to_snake_case(&name).to_uppercase()
            })),
        }
        .and_then(|env_key| {
            let val = var(env_key).ok()?;
            match serde_plain::from_str(&val) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        found = val,
                        "Found env value but could not parse it as {}. {e}",
                        type_name::<T>()
                    );
                    None
                }
            }
        });
        Self {
            default: val,
            config: None,
            cli: None,
            env,
        }
    }

    /// Setting value with respect to it's source priority
    pub fn customized(&self) -> &T {
        self.cli
            .as_ref()
            .or(self.env.as_ref())
            .or(self.config.as_ref())
            .unwrap_or(&self.default)
    }
}

pub trait AnySettingValue: 'static + Send + Sync {
    fn key(&self) -> String;
    fn require_restart(&self) -> bool;
    fn type_name(&self) -> std::borrow::Cow<'static, str>;

    fn customized_value(&self) -> &dyn Any;
    fn config_mut(&mut self) -> &mut dyn Any;
    fn cli_mut(&mut self) -> &mut dyn Any;
    fn reset_config_value(&mut self);

    fn serialize_config(&self) -> Option<toml::Value>;
    fn serialize_response(&self) -> SerializedSetting;

    fn deserialize_toml(&mut self, from: toml::Value) -> Result<(), toml::de::Error>;
    fn deserialize_json(&mut self, from: serde_json::Value) -> Result<(), serde_json::Error>;
}

impl<T: ConfigValue> AnySettingValue for SettingValue<T> {
    fn key(&self) -> String {
        T::KEY
            .map(|k| k.to_string())
            .unwrap_or_else(|| camel_to_snake_case(&self.type_name()))
    }

    fn require_restart(&self) -> bool {
        T::REQUIRE_RESTART
    }

    fn type_name(&self) -> std::borrow::Cow<'static, str> {
        T::name()
    }

    fn deserialize_toml(&mut self, from: toml::Value) -> Result<(), toml::de::Error> {
        let value = T::deserialize(from)?;
        self.config = Some(value);
        Ok(())
    }

    fn deserialize_json(&mut self, json: serde_json::Value) -> Result<(), serde_json::Error> {
        match json {
            serde_json::Value::Null => {
                self.config = None;
            }
            _ => {
                let value = serde_json::from_value(json)?;
                self.config = Some(value);
            }
        }
        Ok(())
    }

    fn serialize_config(&self) -> Option<toml::Value> {
        let value = self.config.clone();
        Some(toml::Value::try_from(value?).unwrap())
    }

    fn serialize_response(&self) -> SerializedSetting {
        let serialize = |t: Option<&T>| serde_json::to_value(t).unwrap();
        SerializedSetting {
            key: self.key(),
            require_restart: T::REQUIRE_RESTART,
            default_value: serialize(Some(&self.default)),
            config_value: serialize(self.config.as_ref()),
            cli_value: serialize(self.cli.as_ref()),
            env_value: serialize(self.env.as_ref()),
        }
    }

    fn customized_value(&self) -> &dyn Any {
        self.customized()
    }

    fn config_mut(&mut self) -> &mut dyn Any {
        &mut self.config
    }

    fn cli_mut(&mut self) -> &mut dyn Any {
        &mut self.cli
    }

    fn reset_config_value(&mut self) {
        self.config = None;
    }
}

pub static CONFIG: LazyLock<ConfigStore> = LazyLock::new(ConfigStore::construct);

type SettingsInnerStore = HashMap<TypeId, Box<dyn AnySettingValue>>;

#[derive(Clone)]
pub struct ConfigStore {
    settings: watch::Sender<SettingsInnerStore>,
}

impl std::fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigStore").finish()
    }
}

impl ConfigStore {
    pub fn new() -> Self {
        let (settings_tx, _) = watch::channel(HashMap::new());
        Self {
            settings: settings_tx,
        }
    }

    pub fn register_value<T: ConfigValue>(&self) {
        let default = T::default();
        self.settings.send_modify(|setting| {
            setting.insert(TypeId::of::<T>(), Box::new(SettingValue::new(default)));
        });
    }

    fn get_t<T: ConfigValue>(inner_storage: &SettingsInnerStore) -> T {
        let setting = inner_storage
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("unregistered setting type {}", type_name::<T>()));
        let t: &T = setting.customized_value().downcast_ref().unwrap();
        t.clone()
    }

    /// Retrieve fresh single setting value.
    pub fn get_value<T: ConfigValue>(&self) -> T {
        let settings = self.settings.borrow();
        Self::get_t(&settings)
    }

    /// Retrieve multiple settings values from the settings storage at once.
    ///
    /// Batch all settings reads under a single read lock
    pub fn get_values<T: ManySettingsExtractor>(&self) -> T {
        let settings = self.settings.borrow();
        T::extract(&settings)
    }

    pub fn update_value<T: ConfigValue>(&self, new: T) {
        self.settings.send_modify(|settings| {
            let setting = settings
                .get_mut(&TypeId::of::<T>())
                .unwrap_or_else(|| panic!("unregistered setting type {}", type_name::<T>()));
            let value = setting.config_mut();
            let value = value.downcast_mut().unwrap();
            *value = Some(new);
        });
    }

    pub fn construct_table(&self) -> toml::Table {
        let mut table = toml::Table::new();
        let settings = self.settings.borrow();
        for setting in settings.values() {
            let Some(value) = setting.serialize_config() else {
                continue;
            };
            table.insert(setting.key(), value);
        }
        table
    }

    pub fn json(&self) -> Vec<SerializedSetting> {
        let settings = self.settings.borrow();
        let mut out = Vec::with_capacity(settings.len());
        for setting in settings.values() {
            // CHANGE FROM ARRAY TO SETTINGS OBJECT?
            let value = setting.serialize_response();
            out.push(value);
        }
        out
    }

    pub fn apply_toml_settings(&self, table: toml::Table) {
        self.settings.send_modify(|settings| {
            for setting in settings.values_mut() {
                let key = setting.key();
                if let Some(val) = table.get(&key).cloned()
                    && let Err(err) = setting.deserialize_toml(val)
                {
                    tracing::warn!(
                        "Failed to deserialize toml value for {}: {err}",
                        setting.type_name()
                    )
                };
            }
        });
    }

    pub fn apply_json(&self, value: serde_json::Value) -> crate::Result<ConfigurationApplyResult> {
        let mut result = ConfigurationApplyResult::default();
        let obj = match value {
            serde_json::Value::Object(obj) => obj,
            _ => return Err(AppError::bad_request("Provided json must be object")),
        };

        self.settings.send_modify(|settings| {
            for setting in settings.values_mut() {
                if let Some(val) = obj.get(&setting.key()).cloned() {
                    match setting.deserialize_json(val) {
                        Ok(_) if setting.require_restart() => result.require_restart = true,
                        Ok(_) => (),
                        Err(err) => {
                            tracing::warn!(
                                "Failed to deserialize json value for {}: {err}",
                                setting.type_name()
                            );
                            result.errors.push(ConfigurationApplyError {
                                key: setting.key(),
                                message: err.to_string(),
                            });
                        }
                    };
                }
            }
        });
        Ok(result)
    }

    pub fn apply_config_value<T: ConfigValue>(&self, value: T) {
        self.settings.send_modify(|settings| {
            let setting = settings.get_mut(&value.type_id()).unwrap();
            let setting = setting.config_mut();
            let val = setting.downcast_mut().unwrap();
            *val = Some(value);
        });
    }

    pub fn apply_cli_value<T: ConfigValue>(&self, value: T) {
        self.settings.send_modify(|settings| {
            let setting = settings.get_mut(&value.type_id()).unwrap();
            let setting = setting.cli_mut();
            let val = setting.downcast_mut().unwrap();
            *val = Some(value);
        });
    }

    pub fn reset_config_values(&self) {
        self.settings.send_modify(|settings| {
            for setting in settings.values_mut() {
                setting.reset_config_value();
            }
        });
    }

    pub fn watch_value<T: ConfigValue>(&self) -> ConfigValueWatcher<T> {
        let rx = self.settings.subscribe();
        let current_value = self.get_value::<T>();
        ConfigValueWatcher {
            current_value,
            t_id: std::any::TypeId::of::<T>(),
            rx,
        }
    }
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ConfigValueWatcher<T> {
    rx: watch::Receiver<SettingsInnerStore>,
    current_value: T,
    t_id: std::any::TypeId,
}

impl<T: ConfigValue + PartialEq> ConfigValueWatcher<T> {
    /// Future resolves with the new value when it changes
    /// Cancellation safe
    pub async fn watch_change(&mut self) -> T {
        let changed_config = self
            .rx
            .wait_for(|map| {
                let val = map.get(&self.t_id).expect("config values be registered");
                let new = val.customized_value().downcast_ref::<T>().unwrap();
                *new != self.current_value
            })
            .await
            .expect("config is static so channel is never dropped");
        let new_value = changed_config
            .get(&self.t_id)
            .unwrap()
            .customized_value()
            .downcast_ref::<T>()
            .unwrap()
            .clone();
        self.current_value = new_value.clone();
        new_value
    }

    pub fn current_value(&self) -> &T {
        &self.current_value
    }

    pub fn has_changed(&self) -> bool {
        self.rx
            .has_changed()
            .expect("config is static so channel is never dropped")
    }
}

// Shady utoipa manual implementation

impl<T: ConfigValue> utoipa::ToSchema for UtoipaConfigValue<T> {
    fn name() -> std::borrow::Cow<'static, str> {
        T::name()
    }
}

impl<T: ConfigValue> utoipa::PartialSchema for UtoipaConfigValue<T> {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema;
        let name = T::name();
        let inner_schema = T::schema();
        let snake_name = camel_to_snake_case(&name);
        let optional: RefOr<utoipa::openapi::Schema> = match &inner_schema {
            RefOr::T(schema::Schema::Object(obj)) => {
                let obj = obj.clone();
                obj.into()
            }
            RefOr::T(schema::Schema::Array(obj)) => {
                let obj = obj.clone();
                obj.into()
            }
            RefOr::T(schema) => match schema {
                schema::Schema::Array(_) => panic!("Can't handle array schema type"),
                schema::Schema::Object(_) => panic!("Can't handle object schema type"),
                schema::Schema::OneOf(_) => panic!("Can't handle one_of schema type"),
                schema::Schema::AllOf(_) => panic!("Can't handle all_of schema type"),
                schema::Schema::AnyOf(_) => panic!("Can't handle any_of schema type"),
                _ => panic!("Can't handle other schema type"),
            },
            RefOr::Ref(r) => RefOr::Ref(r.clone()),
        };
        let key = T::KEY.unwrap_or(&snake_name);
        let key_schema = schema::ObjectBuilder::new()
            .schema_type(schema::SchemaType::Type(schema::Type::String))
            .enum_values(Some([key]));

        schema::ObjectBuilder::new()
            .schema_type(schema::SchemaType::Type(schema::Type::Object))
            .property("require_restart", bool::schema())
            .required("require_restart")
            .property("key", key_schema)
            .required("key")
            .property("default_value", inner_schema.clone())
            .required("default_value")
            .property("config_value", optional.clone())
            .required("config_value")
            .property("cli_value", optional.clone())
            .required("cli_value")
            .property("env_value", optional)
            .required("env_value")
            .into()
    }
}

impl utoipa::ToSchema for UtoipaConfigSchema {
    fn name() -> std::borrow::Cow<'static, str> {
        "ConfigSchema".into()
    }
}

#[derive(Debug)]
pub struct UtoipaConfigValue<T> {
    _t: std::marker::PhantomData<T>,
}

#[derive(Debug)]
pub struct UtoipaConfigSchema;

/// Trait that allows extracting multiple settings values using a single borrow inside generic tuple
pub trait ManySettingsExtractor {
    fn extract(storage: &SettingsInnerStore) -> Self;
}

macro_rules! impl_many_settings_extractor_for_tuples {
    () => {};

    ($(($($types:ident),*)),*) => {
        $(
            impl<$($types: ConfigValue + 'static),*> ManySettingsExtractor for ($($types,)*) {
                fn extract(storage: &SettingsInnerStore) -> Self {
                    ($(
                        ConfigStore::get_t::<$types>(storage),
                    )*)
                }
            }
        )*
    };
}

impl_many_settings_extractor_for_tuples! {
    (A),
    (A, B),
    (A, B, C),
    (A, B, C, D),
    (A, B, C, D, E),
    (A, B, C, D, E, F),
    (A, B, C, D, E, F, G),
    (A, B, C, D, E, F, G, H),
    (A, B, C, D, E, F, G, H, I),
    (A, B, C, D, E, F, G, H, I, J),
    (A, B, C, D, E, F, G, H, I, J, K),
    (A, B, C, D, E, F, G, H, I, J, K, L)
}
