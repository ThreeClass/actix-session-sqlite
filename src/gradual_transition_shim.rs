use actix_session::storage::{CookieSessionStore, LoadError, SaveError, SessionKey, SessionStore, UpdateError};
use actix_web::cookie::time::Duration;
use anyhow::{anyhow, Error};
use crate::{SessionState, SqliteSessionStore, Uuid};

pub struct CookieToSqliteSessionStoreShim {
	primary: SqliteSessionStore,
	secondary: CookieSessionStore
}

impl SessionStore for CookieToSqliteSessionStoreShim {
	async fn load(&self, session_key: &SessionKey) -> Result<Option<SessionState>, LoadError> {
		match Uuid::try_from(session_key.as_ref()) {
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
		match Uuid::try_from(session_key.as_ref()) {
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
		match Uuid::try_from(session_key.as_ref()) {
			Ok(_) => self.primary.update_ttl(session_key, ttl).await,
			Err(_) => Ok(()) //NOOP
			
		}
	}
			
	async fn delete(&self, session_key: &SessionKey) -> Result<(), Error> {
		match Uuid::try_from(session_key.as_ref()) {
			Ok(_) => self.primary.delete(session_key).await,
			Err(_) => Ok(()) //NOOP
		}
	}
}