use std::collections::HashMap;
use std::ops::Add;
use actix_session::storage::{LoadError, SaveError, SessionKey, SessionStore, UpdateError};
use actix_web::cookie::time::{Duration};
use anyhow::{ Error};
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use hex::FromHexError;
use serde_json::Value;
use sqlx::{query, query_as, query_scalar, SqlitePool};
use tracing::{info_span, instrument};
use tracing_futures::Instrument;

pub type SessionState = HashMap<String, String>;
pub struct SqliteSessionStore (pub SqlitePool);

struct DbSessionRow {
	pub id: i64,
	pub expires: DateTime<Utc>,
	pub created: DateTime<Utc>,
	pub data: Value
}

fn convert_duration(duration: &Duration) -> TimeDelta {
	assert!(!duration.is_negative());
	TimeDelta::new(duration.whole_seconds(), duration.subsec_nanoseconds() as u32).expect("Expected valid time")
}

fn decode_key(sess_key: &SessionKey) -> Result<i64, FromHexError> {
	let mut buff = [0u8; 8];
	hex::decode_to_slice(sess_key.as_ref(), &mut buff)?;
	Ok(i64::from_le_bytes(buff))
}

fn encode_key(key: i64) -> SessionKey {
	SessionKey::try_from(hex::encode(key.to_le_bytes())).unwrap()
}

impl SessionStore for SqliteSessionStore {

	#[instrument(skip(self),err)]
	async fn load(&self, session_key: &SessionKey) -> Result<Option<SessionState>, LoadError> {
		let key = decode_key(session_key).map_err(|e| LoadError::Other(Error::from(e)))?;
		let mut t = self.0.begin().instrument(info_span!("Connecting to DB")).await.map_err(|e| LoadError::Other(Error::from(e)))?;
		let     row = query_as!(DbSessionRow, "select * from sessions where id=$1", key).fetch_optional(&mut *t)
			.instrument(info_span!("Querying data"))
			.await.map_err(|e|  LoadError::Other(Error::from(e)))?;
		if row.as_ref().is_some_and(|x| x.expires < Utc::now()) {
			//In case we've queried an expired row, drop it
			query!("delete from sessions where id=$1", key).execute(&mut *t)
				.instrument(info_span!("Dropping timed-out session"))
				.await.map_err(|e|  LoadError::Other(Error::from(e)))?;
			t.commit().await.map_err(|e| LoadError::Other(Error::from(e)))?;
			return Ok(None)
		}
		row.map(|x| serde_json::from_value::<SessionState>(x.data)).transpose().map_err(|e| LoadError::Deserialization(Error::from(e)))
	}

	#[instrument(skip(self), err)]
	async fn save(&self, session_state: SessionState, ttl: &Duration) -> Result<SessionKey, SaveError> {
		let value = serde_json::to_value(session_state).map_err(|e| SaveError::Serialization(Error::from(e)))?;
		let now = Utc::now();
		let expires = now.add(convert_duration(ttl));
		let id = query_scalar!("insert into sessions (created, expires, data) values ($1, $2, $3) returning id",
			now, expires, value)
			.fetch_one(&self.0)
			.await.map_err(|e|  SaveError::Other(Error::from(e)))?;
		Ok(encode_key(id))
	}

	#[instrument(skip(self), err)]
	async fn update(&self, session_key: SessionKey, session_state:SessionState, ttl: &Duration) -> Result<SessionKey, UpdateError> {
		let key = decode_key(&session_key).map_err(|e| UpdateError::Other(Error::from(e)))?;
		let value = serde_json::to_value(session_state).map_err(|e| UpdateError::Serialization(Error::from(e)))?;
		let expires =  Utc::now().add(convert_duration(ttl));
		query!("update sessions set data=$2, expires=$3 where id=$1", key, value, expires).execute(&self.0).await
			.map_err(|e| UpdateError::Other(Error::from(e)))?;

		Ok(session_key)
	}

	#[instrument(skip(self), err)]
	async fn update_ttl(&self, session_key: &SessionKey, ttl: &Duration) -> Result<(), Error> {
		let key = decode_key(&session_key).map_err(|e| UpdateError::Other(Error::from(e)))?;
		let expires =  Utc::now().add(convert_duration(ttl));
		query!("update sessions set expires=$2 where id=$1", key, expires).execute(&self.0).await
			.map_err(|e| UpdateError::Other(Error::from(e)))?;

		Ok(())
	}

	#[instrument(skip(self), err)]
	async fn delete(&self, session_key: &SessionKey) -> Result<(), Error> {
		let key = decode_key(&session_key).map_err(|e| Error::from(e))?;
		query!("delete from sessions where id=$1", key).execute(&self.0)
			.await.map_err(|e|  LoadError::Other(Error::from(e)))?;
		Ok(())
	}
}