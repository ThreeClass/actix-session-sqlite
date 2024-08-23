mod gradual_transition_shim;

use std::collections::HashMap;
use std::convert::From;
use std::str::FromStr;
use actix_session::storage::{LoadError, SaveError, SessionKey, SessionStore, UpdateError};
use actix_web::cookie::time::{Duration};
use anyhow::{Error};
use chrono::{NaiveDateTime, TimeDelta, Utc};
use sqlx::{migrate, query, query_as, query_scalar, SqlitePool};
use tracing::{info_span, instrument};
use tracing_futures::Instrument;
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use uuid::Uuid;

pub type SessionState = HashMap<String, String>;
pub(crate) type StoreSessionKey = Uuid;
pub struct SqliteSessionStore (pub SqlitePool);

struct DbSessionRow {
	#[allow(dead_code)]
	pub id: Uuid,
	pub expires: NaiveDateTime,
	#[allow(dead_code)]
	pub created: NaiveDateTime,
	pub data: Value
}

fn convert_duration(duration: &Duration) -> TimeDelta {
	assert!(!duration.is_negative());
	TimeDelta::new(duration.whole_seconds(), duration.subsec_nanoseconds() as u32).expect("Expected valid time")
}

impl SqliteSessionStore {
	#[instrument(skip(self), err)]
	pub async fn clean_database(&self ) -> Result<u32, sqlx::Error> {
		let mut t = self.0.begin().instrument(info_span!("Connecting to DB")).await?;
		query!("delete from sessions where strftime('%s', expires) < unixepoch()").execute(&mut *t).instrument(info_span!("Deleting data")).await?;
		let result = query_scalar!(r#"select changes() as "foo!:u32" from sessions"#).fetch_one(&mut *t).instrument(info_span!("Querying changes")).await?;
		t.commit().await?;
		Ok(result)
	}

	//Recommended: sqlite:///var/lib/service/session.db?mode=rwc
	pub async fn open_with_path(path: &str) -> Result<Self, Error> {
		let pool = SqlitePool::connect_with(SqliteConnectOptions::from_str(path)?).await?;
		migrate!().run(&pool).await?;
		Ok(Self(pool))
	}
}

impl SessionStore for SqliteSessionStore {
	#[instrument(skip(self), err)]
	async fn load(&self, session_key: &SessionKey) -> Result<Option<SessionState>, LoadError> {
		let key= StoreSessionKey::try_from(session_key.as_ref()).map_err(|e| LoadError::Other(Error::from(e)))?;
		let mut t = self.0.begin().instrument(info_span!("Connecting to DB")).await.map_err(|e| LoadError::Other(Error::from(e)))?;
		let row = query_as!(DbSessionRow, r#"select id as "id!: Uuid", expires, created, data as "data: Value" from sessions where id=$1"#, key).fetch_optional(&mut *t)
			.instrument(info_span!("Querying data"))
			.await.map_err(|e|  LoadError::Other(Error::from(e)))?;
		if row.as_ref().is_some_and(|x| x.expires < Utc::now().naive_utc()) {
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
		let new_key = Uuid::now_v7();
		let now = Utc::now();
		let expires = now + convert_duration(ttl);
		let id = query_scalar!(r#"insert into sessions (id, created, expires, data) values ($1, $2, $3, $4) returning id as "id!: Uuid""#,
			new_key, now, expires, value)
			.fetch_one(&self.0)
			.await.map_err(|e|  SaveError::Other(Error::from(e)))?;
		Ok(id.hyphenated().to_string().try_into().expect(""))
	}

	#[instrument(skip(self), err)]
	async fn update(&self, session_key: SessionKey, session_state:SessionState, ttl: &Duration) -> Result<SessionKey, UpdateError> {
		let key = StoreSessionKey::try_from(session_key.as_ref()).map_err(|e| UpdateError::Other(Error::from(e)))?;
		let value = serde_json::to_value(session_state).map_err(|e| UpdateError::Serialization(Error::from(e)))?;
		let expires =  Utc::now() + convert_duration(ttl);
		query!("update sessions set data=$2, expires=$3 where id=$1", key, value, expires).execute(&self.0).await
			.map_err(|e| UpdateError::Other(Error::from(e)))?;

		Ok(session_key)
	}

	#[instrument(skip(self), err)]
	async fn update_ttl(&self, session_key: &SessionKey, ttl: &Duration) -> Result<(), Error> {
		let key = StoreSessionKey::try_from(session_key.as_ref()).map_err(|e| UpdateError::Other(Error::from(e)))?;
		let expires =  Utc::now() + convert_duration(ttl);
		query!("update sessions set expires=$2 where id=$1", key, expires).execute(&self.0).await
			.map_err(|e| UpdateError::Other(Error::from(e)))?;

		Ok(())
	}

	#[instrument(skip(self), err)]
	async fn delete(&self, session_key: &SessionKey) -> Result<(), Error> {
		let key = StoreSessionKey::try_from(session_key.as_ref()).map_err(|e| UpdateError::Other(Error::from(e)))?;
		query!("delete from sessions where id=$1", key).execute(&self.0)
			.await.map_err(|e|  LoadError::Other(Error::from(e)))?;
		Ok(())
	}
}

#[cfg(test)]
mod test {
	use std::collections::HashMap;
	use actix_session::storage::SessionStore;
	use actix_web::cookie::time::Duration;
	use crate::{SqliteSessionStore, Uuid};
	

	#[tokio::test]
	async fn test_one() {

		let sess = SqliteSessionStore::open_with_path(":memory:").await.expect("");
		
		let data1 = HashMap::from([("1".to_string(), "loremp".to_string()), ("2".to_string(), "Ipsum".to_string())]);

		let key1 = sess.save(data1.clone(), &Duration::hours(1)).await.unwrap();
		println!("{:?}", key1);
		let key2 = sess.save(data1, &Duration::hours(1)).await.unwrap();

		println!("{:?}", Uuid::parse_str(key1.as_ref()).expect("").get_timestamp());
		println!("{:?}", key2);

		assert_ne!(key1, key2);

		//TODO: test delete, update_ttl,...
	}
}