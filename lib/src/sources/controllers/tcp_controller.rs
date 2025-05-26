use crate::errors::ControllerError;
use crate::sources::DataSourceT;
use crate::sources::controllers::Controller;
use std::net::SocketAddr;

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, schemars::JsonSchema, Default)]
#[serde(default)]
pub struct TCPControllerParams {}

#[typetag::serde(name = "TCP")]
#[async_trait::async_trait]
impl Controller for TCPControllerParams {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        Ok(())
    }
}
