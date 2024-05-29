use crate::ServerConfiguration;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

pub trait ReplaySaving {
    fn save_replay_data(
        &mut self,
        config: &ServerConfiguration,
        replay_data: Bytes,
        start_time: DateTime<Utc>,
    );
}

pub struct FileReplaySaving {
    directory: PathBuf,
}

impl FileReplaySaving {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

impl ReplaySaving for FileReplaySaving {
    fn save_replay_data(
        &mut self,
        config: &ServerConfiguration,
        replay_data: Bytes,
        start_time: DateTime<Utc>,
    ) {
        let time = start_time.format("%Y-%m-%dT%H%M%S").to_string();
        let file_name = format!("{}.{}.hrp", config.server_name, time);
        let directory = self.directory.clone();
        let path = self.directory.join(&file_name);

        tokio::spawn(async move {
            if tokio::fs::create_dir_all(directory).await.is_err() {
                return;
            };

            let mut file_handle = match File::create(path).await {
                Ok(file) => file,
                Err(_) => {
                    return;
                }
            };

            let _x = file_handle.write(&replay_data).await;
            let _x = file_handle.sync_all().await;
        });
    }
}

pub struct HttpEndpointReplaySaving {
    url: String,
    client: reqwest::Client,
}

impl HttpEndpointReplaySaving {
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }
}

impl ReplaySaving for HttpEndpointReplaySaving {
    fn save_replay_data(
        &mut self,
        config: &ServerConfiguration,
        replay_data: Bytes,
        start_time: DateTime<Utc>,
    ) {
        let client = self.client.clone();
        let server_name = config.server_name.clone();
        let time = start_time.format("%Y-%m-%dT%H%M%S").to_string();
        let file_name = format!("{}.{}.hrp", config.server_name, time);
        let form = reqwest::multipart::Form::new()
            .text("time", time)
            .text("server", server_name)
            .part(
                "replay",
                reqwest::multipart::Part::stream(replay_data).file_name(file_name),
            );

        let request = client.post(&self.url).multipart(form);
        tokio::spawn(async move {
            let _x = request.send().await;
        });
    }
}
