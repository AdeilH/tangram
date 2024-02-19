use std::{collections::VecDeque, sync::Arc};

pub struct Pool<T> {
	semaphore: Arc<tokio::sync::Semaphore>,
	objects: Arc<tokio::sync::Mutex<VecDeque<T>>>,
	sender: tokio::sync::mpsc::UnboundedSender<T>,
}

pub struct Guard<T> {
	permit: Option<tokio::sync::OwnedSemaphorePermit>,
	object: Option<T>,
	sender: tokio::sync::mpsc::UnboundedSender<T>,
}

impl<T> Pool<T>
where
	T: Send + 'static,
{
	#[allow(clippy::new_without_default)]
	#[must_use]
	pub fn new() -> Self {
		let semaphore = Arc::new(tokio::sync::Semaphore::new(0));
		let objects = Arc::new(tokio::sync::Mutex::new(VecDeque::new()));
		let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
		tokio::spawn({
			let semaphore = semaphore.clone();
			let objects = objects.clone();
			async move {
				while let Some(object) = receiver.recv().await {
					objects.lock().await.push_back(object);
					semaphore.add_permits(1);
				}
			}
		});
		Self {
			semaphore,
			objects,
			sender,
		}
	}

	pub async fn get(&self) -> Guard<T> {
		let permit = self.semaphore.clone().acquire_owned().await.unwrap();
		let object = self.objects.lock().await.pop_front().unwrap();
		Guard {
			permit: Some(permit),
			object: Some(object),
			sender: self.sender.clone(),
		}
	}

	pub async fn put(&self, object: T) {
		self.objects.lock().await.push_back(object);
		self.semaphore.add_permits(1);
	}
}

impl<T> Guard<T> {
	pub fn replace(&mut self, object: T) -> T {
		self.object.replace(object).unwrap()
	}

	pub fn take(mut self) -> T {
		self.permit.take().unwrap().forget();
		self.object.take().unwrap()
	}
}

impl<T> std::ops::Deref for Guard<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		self.object.as_ref().unwrap()
	}
}

impl<T> std::ops::DerefMut for Guard<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.object.as_mut().unwrap()
	}
}

impl<T> Drop for Guard<T> {
	fn drop(&mut self) {
		if let Some(object) = self.object.take() {
			self.permit.take().unwrap().forget();
			self.sender.send(object).ok();
		}
	}
}
