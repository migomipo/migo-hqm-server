use crate::ServerConfiguration;
use bytes::Bytes;
use futures::future::BoxFuture;
use jiff::Timestamp;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tracing::warn;

pub trait RecordingSaveMethod {
    /// Returns a future that saves the replay. The server runs it in the background, and waits
    /// for it to finish before it shuts down.
    fn save_recording_data(
        &mut self,
        config: &ServerConfiguration,
        replay_data: Bytes,
        start_time: Timestamp,
    ) -> BoxFuture<'static, ()>;
}

pub struct RecordingSaveToFile {
    directory: PathBuf,
}

impl RecordingSaveToFile {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

impl RecordingSaveMethod for RecordingSaveToFile {
    fn save_recording_data(
        &mut self,
        config: &ServerConfiguration,
        replay_data: Bytes,
        start_time: Timestamp,
    ) -> BoxFuture<'static, ()> {
        let time = start_time.strftime("%Y-%m-%dT%H%M%S").to_string();
        let file_name = format!("{}.{}.hrp", config.server_name, time);
        let directory = self.directory.clone();
        let path = self.directory.join(&file_name);

        Box::pin(async move {
            let result = async {
                tokio::fs::create_dir_all(directory).await?;
                let mut file_handle = tokio::fs::File::create(&path).await?;
                file_handle.write_all(&replay_data).await?;
                file_handle.sync_all().await
            }
            .await;
            if let Err(e) = result {
                warn!("Could not save replay {}: {e}", path.display());
            }
        })
    }
}

pub struct RecordingSendToHttpEndpoint {
    url: String,
    client: reqwest::Client,
}

impl RecordingSendToHttpEndpoint {
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }
}

impl RecordingSaveMethod for RecordingSendToHttpEndpoint {
    fn save_recording_data(
        &mut self,
        config: &ServerConfiguration,
        replay_data: Bytes,
        start_time: Timestamp,
    ) -> BoxFuture<'static, ()> {
        let client = self.client.clone();
        let server_name = config.server_name.clone();
        let time = start_time.strftime("%Y-%m-%dT%H%M%S").to_string();
        let file_name = format!("{}.{}.hrp", config.server_name, time);
        let form = reqwest::multipart::Form::new()
            .text("time", time)
            .text("server", server_name)
            .part(
                "replay",
                reqwest::multipart::Part::stream(replay_data).file_name(file_name),
            );

        let request = client.post(&self.url).multipart(form);
        Box::pin(async move {
            if let Err(e) = request.send().await.and_then(|r| r.error_for_status()) {
                warn!("Could not send replay to the replay endpoint: {e}");
            }
        })
    }
}
