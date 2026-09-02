//! The gRPC transport adapter `apicli` uses to talk to `apid`'s `Engine`
//! service: a thin wrapper around the generated `EngineClient` so CLI code
//! never touches `tonic`'s `Request`/`Channel`/`Status` directly. Mirrors
//! `api/grpc_api`'s role on the server side of the same RPC - this crate's
//! only public surface is [`EngineGrpcClient`].

use engine_entities::engine::{
    engine_client::EngineClient, GetRunResultRequest, GetRunResultResponse, GetSerivceRequest,
    GetServiceResponse, ListRequest, RunServiceRequest, SaveServiceRequest,
};
use tonic::{transport::Channel, Request};

pub use tonic::{transport::Error as ConnectError, Status};

/// A typed gRPC client to `apid`'s `Engine` service.
pub struct EngineGrpcClient {
    /// The underlying generated client.
    inner: EngineClient<Channel>,
}

impl EngineGrpcClient {
    /// Connects to `apid` at `endpoint` (e.g. `"http://host:port"`).
    ///
    /// # Errors
    /// Returns [`ConnectError`] if the connection can't be established.
    #[inline]
    pub async fn connect(endpoint: String) -> Result<Self, ConnectError> {
        let inner = EngineClient::connect(endpoint).await?;

        Ok(Self { inner })
    }

    /// Lists every operation of every loaded service, as the display names
    /// `apid` renders for them (e.g. `"(swagger) petstore.listPets"`).
    ///
    /// # Errors
    /// Returns [`Status`] if the RPC fails.
    #[inline]
    pub async fn list(&mut self) -> Result<Vec<String>, Status> {
        let response = self
            .inner
            .list(Request::new(ListRequest {}))
            .await?
            .into_inner();

        Ok(response.items.into_iter().map(|item| item.name).collect())
    }

    /// Fetches a service's raw manifest bytes, and raw credential bytes if
    /// it has any.
    ///
    /// # Errors
    /// Returns [`Status`] if the RPC fails.
    #[inline]
    pub async fn get_service(&mut self, name: String) -> Result<GetServiceResponse, Status> {
        let request = Request::new(GetSerivceRequest { name });

        Ok(self.inner.get_service(request).await?.into_inner())
    }

    /// Saves a service's manifest and/or credentials, as raw bytes.
    ///
    /// # Errors
    /// Returns [`Status`] if the RPC fails.
    #[inline]
    pub async fn save_service(
        &mut self,
        name: String,
        raw_service: Option<Vec<u8>>,
        raw_credentials: Option<Vec<u8>>,
    ) -> Result<(), Status> {
        let request = Request::new(SaveServiceRequest {
            name,
            raw_service,
            raw_credentials,
        });

        self.inner.save_service(request).await?;

        Ok(())
    }

    /// Starts running `id` (a `"{service}.{operation}"` identifier) against
    /// `input`, capped at `limit` results, and returns the resulting
    /// execution ID.
    ///
    /// # Errors
    /// Returns [`Status`] if the RPC fails.
    #[inline]
    pub async fn run_service(
        &mut self,
        id: String,
        input: String,
        limit: Option<i32>,
    ) -> Result<String, Status> {
        let request = Request::new(RunServiceRequest {
            id,
            input,
            limit,
            execution_id: None,
        });

        let response = self.inner.run_service(request).await?.into_inner();

        Ok(response.execution_id)
    }

    /// Fetches a run's current result, or status if it hasn't completed.
    ///
    /// # Errors
    /// Returns [`Status`] if the RPC fails.
    #[inline]
    pub async fn get_run_result(
        &mut self,
        execution_id: String,
    ) -> Result<GetRunResultResponse, Status> {
        let request = Request::new(GetRunResultRequest { execution_id });

        Ok(self.inner.get_run_result(request).await?.into_inner())
    }
}
