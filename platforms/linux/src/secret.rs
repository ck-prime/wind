//! Linux Secret Service.
//!
//! Secrets go through the session Secret Service when it is available. A
//! missing bus or service is an error. This file is not compiled or
//! runtime-tested by a Windows build. There is no plaintext file fallback.

use std::collections::HashMap;

use wind_storage::{check_name, report_secret_unavailable, SecretError, SecretStore};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

const DESTINATION: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const COLLECTION_PATH: &str = "/org/freedesktop/secrets/aliases/default";
const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_INTERFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";

/// Session Secret Service store.
#[derive(Debug, Default)]
pub struct LinuxSecretStore;

impl SecretStore for LinuxSecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        check_name(name)?;
        let connection = session()?;
        let service = proxy(&connection, SERVICE_PATH, SERVICE_INTERFACE)?;
        let session_path = open_session(&service)?;
        let Some(item) = search(&service, name)? else {
            return Ok(None);
        };
        let item_proxy = proxy(&connection, item.as_str(), ITEM_INTERFACE)?;
        let secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String) = item_proxy
            .call("GetSecret", &(session_path,))
            .map_err(|_| unavailable("secret service could not be read"))?;
        Ok(Some(secret.2))
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        check_name(name)?;
        if value.is_empty() {
            return Err(SecretError::rejected("secret value is empty"));
        }
        self.delete(name)?;
        let connection = session()?;
        let service = proxy(&connection, SERVICE_PATH, SERVICE_INTERFACE)?;
        let session_path = open_session(&service)?;
        let collection = proxy(&connection, COLLECTION_PATH, COLLECTION_INTERFACE)?;
        let mut properties = HashMap::new();
        properties.insert(
            "org.freedesktop.Secret.Item.Label".to_string(),
            Value::from(format!("com.wind.agent {name}")),
        );
        properties.insert(
            "org.freedesktop.Secret.Item.Attributes".to_string(),
            Value::from(attributes(name)),
        );
        let secret = (
            session_path,
            Vec::<u8>::new(),
            value.to_vec(),
            "application/octet-stream".to_string(),
        );
        let (_item, _prompt): (OwnedObjectPath, OwnedObjectPath) = collection
            .call("CreateItem", &(properties, secret, true))
            .map_err(|_| unavailable("secret service could not be written"))?;
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        check_name(name)?;
        let connection = session()?;
        let service = proxy(&connection, SERVICE_PATH, SERVICE_INTERFACE)?;
        let Some(item) = search(&service, name)? else {
            return Ok(());
        };
        let item_proxy = proxy(&connection, item.as_str(), ITEM_INTERFACE)?;
        let _prompt: OwnedObjectPath = item_proxy
            .call("Delete", &())
            .map_err(|_| unavailable("secret service could not be updated"))?;
        Ok(())
    }
}

fn session() -> Result<Connection, SecretError> {
    Connection::session().map_err(|_| unavailable("secret service is unavailable"))
}

fn proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> Result<Proxy<'a>, SecretError> {
    let path =
        ObjectPath::try_from(path).map_err(|_| unavailable("secret service path is invalid"))?;
    Proxy::new(connection, DESTINATION, path, interface)
        .map_err(|_| unavailable("secret service is unavailable"))
}

fn open_session(service: &Proxy<'_>) -> Result<OwnedObjectPath, SecretError> {
    let input = Value::from("");
    let (_output, session): (Value<'_>, OwnedObjectPath) = service
        .call("OpenSession", &("plain", &input))
        .map_err(|_| unavailable("secret service session could not be opened"))?;
    Ok(session)
}

fn search(service: &Proxy<'_>, name: &str) -> Result<Option<OwnedObjectPath>, SecretError> {
    let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
        .call("SearchItems", &(attributes(name),))
        .map_err(|_| unavailable("secret service could not be read"))?;
    Ok(unlocked.into_iter().next())
}

fn attributes(name: &str) -> HashMap<String, String> {
    let mut attributes = HashMap::new();
    attributes.insert("application".to_string(), "com.wind.agent".to_string());
    attributes.insert("name".to_string(), name.to_string());
    attributes
}

fn unavailable(reason: &str) -> SecretError {
    report_secret_unavailable();
    SecretError::unavailable(reason)
}
