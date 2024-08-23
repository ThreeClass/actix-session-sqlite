#![feature(lazy_cell)]

mod gradual_transition_shim;

use std::collections::HashMap;
use std::convert::From;
use actix_session::storage::{LoadError, SaveError, SessionKey, SessionStore, UpdateError};
use actix_web::cookie::time::{Duration};
use anyhow::{ Error};
use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use sqlx::{query, query_as, query_scalar, Database, Decode, Encode, Sqlite, SqlitePool, Type};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::SqliteArgumentValue::Text;
use tracing::{info_span, instrument};
use tracing_futures::Instrument;
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type SessionState = HashMap<String, String>;
pub struct SqliteSessionStore (pub SqlitePool);

//Effectively uuid but fun!
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Uuid {
	#[serde(with = "chrono::serde::ts_milliseconds", rename="t")]
	timestamp: DateTime<Utc>,
	#[serde(rename="r")]
	random: u64
}

impl Uuid {
	fn new() -> Uuid {
		Uuid {
			timestamp: Utc::now(),
			random: random()
		}
	}
}

impl TryFrom<&str> for Uuid {
	type Error = serde_json::Error;

	fn try_from(value: &str) -> Result<Self, Self::Error> {
		serde_json::from_str(value)
	}
}

impl From<Uuid> for String {
	fn from(val: Uuid) -> Self {
		serde_json::to_string(&val).expect("")
	}
}

impl From<Uuid> for DateTime<Utc> {
	fn from(value: Uuid) -> Self {
		value.timestamp
	}
}

impl From<Uuid> for SessionKey {
	fn from(value: Uuid) -> Self {
		SessionKey::try_from(Into::<String>::into(value)).expect("")
	}
}

impl Type<Sqlite> for Uuid {
	fn type_info() -> <Sqlite as Database>::TypeInfo {
		<&String as Type<Sqlite>>::type_info()
	}
}

impl<'q> Encode<'q, Sqlite> for Uuid {
	fn encode_by_ref(&self, buf: &mut <Sqlite as Database>::ArgumentBuffer<'q>) -> Result<IsNull, BoxDynError> {
		let encoded: String = (*self).into();
		buf.push(Text(encoded.into()));
		Ok(IsNull::No)
	}
}

impl<'r> Decode<'r, Sqlite> for Uuid {
	fn decode(value: <Sqlite as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
		let value = <&str as Decode<Sqlite>>::decode(value)?;
		Uuid::try_from(value).map_err(|e| BoxDynError::from(e))
	}
}

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
	async fn clean_database(&self ) -> Result<u32, sqlx::Error> {
		let mut t = self.0.begin().instrument(info_span!("Connecting to DB")).await?;
		query!("delete from sessions where strftime('%s', expires) < unixepoch()").execute(&mut *t).instrument(info_span!("Deleting data")).await?;
		let result = query_scalar!(r#"select changes() as "foo!:u32" from sessions"#).fetch_one(&mut *t).instrument(info_span!("Querying changes")).await?;
		t.commit().await?;
		Ok(result)
	}
}

impl SessionStore for SqliteSessionStore {
	#[instrument(skip(self), err)]
	async fn load(&self, session_key: &SessionKey) -> Result<Option<SessionState>, LoadError> {
		let key= Uuid::try_from(session_key.as_ref()).map_err(|e| LoadError::Other(Error::from(e)))?;
		let mut t = self.0.begin().instrument(info_span!("Connecting to DB")).await.map_err(|e| LoadError::Other(Error::from(e)))?;
		let row = query_as!(DbSessionRow, r#"select id as "id!: Uuid", expires, created,data from sessions where id=$1"#, key).fetch_optional(&mut *t)
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
		let new_key = Uuid::new();
		let now = Utc::now();
		let expires = now + convert_duration(ttl);
		let id = query_scalar!(r#"insert into sessions (id, created, expires, data) values ($1, $2, $3, $4) returning id as "id!: Uuid""#,
			new_key, now, expires, value)
			.fetch_one(&self.0)
			.await.map_err(|e|  SaveError::Other(Error::from(e)))?;
		Ok(id.into())
	}

	#[instrument(skip(self), err)]
	async fn update(&self, session_key: SessionKey, session_state:SessionState, ttl: &Duration) -> Result<SessionKey, UpdateError> {
		let key = Uuid::try_from(session_key.as_ref()).map_err(|e| UpdateError::Other(Error::from(e)))?;
		let value = serde_json::to_value(session_state).map_err(|e| UpdateError::Serialization(Error::from(e)))?;
		let expires =  Utc::now() + convert_duration(ttl);
		query!("update sessions set data=$2, expires=$3 where id=$1", key, value, expires).execute(&self.0).await
			.map_err(|e| UpdateError::Other(Error::from(e)))?;

		Ok(session_key)
	}

	#[instrument(skip(self), err)]
	async fn update_ttl(&self, session_key: &SessionKey, ttl: &Duration) -> Result<(), Error> {
		let key = Uuid::try_from(session_key.as_ref()).map_err(|e| UpdateError::Other(Error::from(e)))?;
		let expires =  Utc::now() + convert_duration(ttl);
		query!("update sessions set expires=$2 where id=$1", key, expires).execute(&self.0).await
			.map_err(|e| UpdateError::Other(Error::from(e)))?;

		Ok(())
	}

	#[instrument(skip(self), err)]
	async fn delete(&self, session_key: &SessionKey) -> Result<(), Error> {
		let key = Uuid::try_from(session_key.as_ref()).map_err(|e| UpdateError::Other(Error::from(e)))?;
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
	use chrono::{DateTime, Utc};
	use sqlx::{migrate, SqlitePool};
	use sqlx::sqlite::SqliteConnectOptions;
	use crate::{SqliteSessionStore, Uuid};

	#[tokio::test]
	async fn test_one() {
		let pool = SqlitePool::connect_with(SqliteConnectOptions::new().filename("debug.db")
			.create_if_missing(true)).await.unwrap();
		migrate!().run(&pool).await.unwrap();
		let sess = SqliteSessionStore(pool);
		
		let data1 = HashMap::from([("1".to_string(), "loremp".to_string()), ("2".to_string(), "Ipsum".to_string())]);

		let key1 = sess.save(data1.clone(), &Duration::hours(1)).await.unwrap();
		println!("{:?}", key1);
		let key2 = sess.save(data1, &Duration::hours(1)).await.unwrap();

		println!("{}", Into::<DateTime<Utc>>::into(Uuid::try_from(key2.as_ref()).expect("")));
		println!("{:?}", key2);

		assert_ne!(key1, key2);
	}
}