use crate::{Error, Outgoing};
use bytes::Bytes;
use futures::{Future, Stream, TryStreamExt as _};

pub trait Ext: Sized {
	#[must_use]
	fn ok(self) -> Self;

	#[must_use]
	fn bad_request(self) -> Self;

	#[must_use]
	fn not_found(self) -> Self;

	fn header_json<K, V>(self, key: K, value: V) -> Result<Self, Error>
	where
		http::HeaderName: TryFrom<K>,
		<http::HeaderName as TryFrom<K>>::Error: Into<http::Error>,
		V: serde::Serialize;

	fn empty(self) -> http::Result<http::Response<Outgoing>>;

	fn bytes<T>(self, value: T) -> http::Result<http::Response<Outgoing>>
	where
		T: Into<Bytes>;

	fn json<T>(self, value: T) -> http::Result<http::Response<Outgoing>>
	where
		T: serde::Serialize + Send + Sync + 'static;

	fn stream<S, T, E>(self, value: S) -> http::Result<http::Response<Outgoing>>
	where
		S: Stream<Item = Result<T, E>> + Send + 'static,
		T: Into<Bytes> + 'static,
		E: Into<Error> + 'static;

	fn future_bytes<F, T, E>(self, value: F) -> http::Result<http::Response<Outgoing>>
	where
		F: Future<Output = Result<T, E>> + Send + 'static,
		T: Into<Bytes> + 'static,
		E: Into<Error> + 'static;

	fn future_json<F, T, E>(self, value: F) -> http::Result<http::Response<Outgoing>>
	where
		F: Future<Output = Result<T, E>> + Send + 'static,
		T: serde::Serialize,
		E: Into<Error> + 'static;

	fn future_optional_json<F, T, E>(self, value: F) -> http::Result<http::Response<Outgoing>>
	where
		F: Future<Output = Result<Option<T>, E>> + Send + 'static,
		T: serde::Serialize,
		E: Into<Error> + 'static;
}

impl Ext for http::response::Builder {
	fn ok(self) -> Self {
		self.status(http::StatusCode::OK)
	}

	fn bad_request(self) -> Self {
		self.status(http::StatusCode::BAD_REQUEST)
	}

	fn not_found(self) -> Self {
		self.status(http::StatusCode::NOT_FOUND)
	}

	fn header_json<K, V>(self, key: K, value: V) -> Result<Self, Error>
	where
		http::HeaderName: TryFrom<K>,
		<http::HeaderName as TryFrom<K>>::Error: Into<http::Error>,
		V: serde::Serialize,
	{
		let value = serde_json::to_string(&value)?;
		Ok(self.header(key, value))
	}

	fn empty(self) -> http::Result<http::Response<Outgoing>> {
		self.body(Outgoing::empty())
	}

	fn bytes<T>(self, value: T) -> http::Result<http::Response<Outgoing>>
	where
		T: Into<Bytes>,
	{
		self.body(Outgoing::bytes(value))
	}

	fn json<T>(self, value: T) -> http::Result<http::Response<Outgoing>>
	where
		T: serde::Serialize + Send + Sync + 'static,
	{
		self.body(Outgoing::json(value))
	}

	fn stream<S, T, E>(self, value: S) -> http::Result<http::Response<Outgoing>>
	where
		S: Stream<Item = Result<T, E>> + Send + 'static,
		T: Into<Bytes> + 'static,
		E: Into<Error> + 'static,
	{
		self.body(Outgoing::stream(value.err_into()))
	}

	fn future_bytes<F, T, E>(self, value: F) -> http::Result<http::Response<Outgoing>>
	where
		F: Future<Output = Result<T, E>> + Send + 'static,
		T: Into<Bytes> + 'static,
		E: Into<Error> + 'static,
	{
		self.body(Outgoing::future_bytes(value))
	}

	fn future_json<F, T, E>(self, value: F) -> http::Result<http::Response<Outgoing>>
	where
		F: Future<Output = Result<T, E>> + Send + 'static,
		T: serde::Serialize,
		E: Into<Error> + 'static,
	{
		self.body(Outgoing::future_json(value))
	}

	fn future_optional_json<F, T, E>(self, value: F) -> http::Result<http::Response<Outgoing>>
	where
		F: Future<Output = Result<Option<T>, E>> + Send + 'static,
		T: serde::Serialize,
		E: Into<Error> + 'static,
	{
		self.body(Outgoing::future_optional_json(value))
	}
}
