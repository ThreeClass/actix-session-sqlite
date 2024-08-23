use actix_session::storage::{CookieSessionStore, LoadError, SaveError, SessionKey, SessionStore, UpdateError};
use actix_web::cookie::time::Duration;
use anyhow::{anyhow, Error};
use crate::{SessionState, SqliteSessionStore, StoreSessionKey};

pub struct CookieToSqliteSessionStoreShim {
	primary: SqliteSessionStore,
	secondary: CookieSessionStore
}
impl CookieToSqliteSessionStoreShim {
	fn new(sqlite: SqliteSessionStore, cookie: CookieSessionStore) -> Self {
		CookieToSqliteSessionStoreShim {
			primary: sqlite,
			secondary: cookie
		}
	}
}

impl SessionStore for CookieToSqliteSessionStoreShim {
	async fn load(&self, session_key: &SessionKey) -> Result<Option<SessionState>, LoadError> {
		match StoreSessionKey::try_from(session_key.as_ref()) {
			Ok(_) => self.primary.load(session_key).await,
			Err(_) => {
				self.secondary.load(session_key).await
			}
		}
	}

	async fn save(&self, session_state: SessionState, ttl: &Duration) -> Result<SessionKey, SaveError> {
		self.primary.save(session_state, ttl).await
	}

	async fn update(&self, session_key: SessionKey, session_state: SessionState, ttl: &Duration) -> Result<SessionKey, UpdateError> {
		match StoreSessionKey::try_from(session_key.as_ref()) {
			Ok(_) => self.primary.update(session_key, session_state, ttl).await,
			Err(_) => {
				//Make sure it's decodable
				let _old_state = self.secondary.load(&session_key).await.map_err(|e| UpdateError::Other(Error::from(e)))?
					.ok_or(UpdateError::Other(anyhow!("")))?;
				
				//And migrate to the new store
				self.primary.save(session_state, ttl).await.map_err(|e| UpdateError::Other(Error::from(e)))
			}
		}
	}

	async fn update_ttl(&self, session_key: &SessionKey, ttl: &Duration) -> Result<(), Error> {
		match StoreSessionKey::try_from(session_key.as_ref()) {
			Ok(_) => self.primary.update_ttl(session_key, ttl).await,
			Err(_) => Ok(()) //NOOP
			
		}
	}
			
	async fn delete(&self, session_key: &SessionKey) -> Result<(), Error> {
		match StoreSessionKey::try_from(session_key.as_ref()) {
			Ok(_) => self.primary.delete(session_key).await,
			Err(_) => Ok(()) //NOOP
		}
	}
}

#[cfg(test)]
mod test {
	use std::collections::HashMap;
	use actix_session::storage::{CookieSessionStore, SessionKey, SessionStore};
	use actix_web::cookie::time::Duration;
	use crate::gradual_transition_shim::CookieToSqliteSessionStoreShim;
	use crate::{SqliteSessionStore, StoreSessionKey};

	#[tokio::test]
	async fn test_shim() {
		let cookie_store = CookieSessionStore::default();

		let mut keys = vec![];

		for i in 0..10 {
			let mp = HashMap::from([("id".to_string(), i.to_string()), ("sqlite".to_string(), "false".to_string())]);
			keys.push(cookie_store.save(mp, &Duration::hours(1)).await.expect(""));
		}

		let sqlite_store = SqliteSessionStore::open_with_path(":memory:").await.expect("");

		for i in 10..20 {
			let mp = HashMap::from([("id".to_string(), i.to_string()), ("sqlite".to_string(), "true".to_string())]);
			keys.push(sqlite_store.save(mp, &Duration::hours(1)).await.expect(""));
		}

		let shim = CookieToSqliteSessionStoreShim::new(sqlite_store, cookie_store);

		for i in 20..30 {
			let mp = HashMap::from([("id".to_string(), i.to_string()), ("sqlite".to_string(), "true".to_string())]);
			keys.push(shim.save(mp, &Duration::hours(1)).await.expect(""));
		}

		//Test for newly inserted
		for i in &keys {
			let val = shim.load(&i).await.expect("").expect("");
			if val.get("sqlite").expect("") == "true" {
				assert!(StoreSessionKey::try_from(i.as_ref()).is_ok())
			}
		}

		for i in (&mut keys).iter().enumerate(){
			let mut v = shim.load(i.1).await.expect("").expect("");
			v.insert("new_store".to_string(), "true".to_string());

			let updated = shim.update(SessionKey::try_from(i.1.as_ref().to_string()).expect(""), v, &Duration::hours(1)).await.expect("");
			assert!(StoreSessionKey::try_from(updated.as_ref()).is_ok());
			println!("Updated: {}: {:?}", i.0, updated);
		}

	}
}