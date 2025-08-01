use crate::{
    error::{RpcError, ServerKind},
    middleware::RethRpcMiddleware,
};
use http::header::AUTHORIZATION;
use jsonrpsee::{
    core::{client::SubscriptionClientT, RegisterMethodError},
    http_client::HeaderMap,
    server::{AlreadyStoppedError, RpcModule},
    ws_client::RpcServiceBuilder,
    Methods,
};
use reth_rpc_api::servers::*;
use reth_rpc_eth_types::EthSubscriptionIdProvider;
use reth_rpc_layer::{
    secret_to_bearer_header, AuthClientLayer, AuthLayer, JwtAuthValidator, JwtSecret,
};
use reth_rpc_server_types::constants;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tower::layer::util::Identity;

pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::server::{ServerConfig, ServerConfigBuilder};
pub use reth_ipc::server::Builder as IpcServerBuilder;

/// Server configuration for the auth server.
#[derive(Debug)]
pub struct AuthServerConfig<RpcMiddleware = Identity> {
    /// Where the server should listen.
    pub(crate) socket_addr: SocketAddr,
    /// The secret for the auth layer of the server.
    pub(crate) secret: JwtSecret,
    /// Configs for JSON-RPC Http.
    pub(crate) server_config: ServerConfigBuilder,
    /// Configs for IPC server
    pub(crate) ipc_server_config: Option<IpcServerBuilder<Identity, Identity>>,
    /// IPC endpoint
    pub(crate) ipc_endpoint: Option<String>,
    /// Configurable RPC middleware
    pub(crate) rpc_middleware: RpcMiddleware,
}

// === impl AuthServerConfig ===

impl AuthServerConfig {
    /// Convenience function to create a new `AuthServerConfig`.
    pub const fn builder(secret: JwtSecret) -> AuthServerConfigBuilder {
        AuthServerConfigBuilder::new(secret)
    }
}
impl<RpcMiddleware> AuthServerConfig<RpcMiddleware> {
    /// Returns the address the server will listen on.
    pub const fn address(&self) -> SocketAddr {
        self.socket_addr
    }

    /// Configures the rpc middleware.
    pub fn with_rpc_middleware<T>(self, rpc_middleware: T) -> AuthServerConfig<T> {
        let Self { socket_addr, secret, server_config, ipc_server_config, ipc_endpoint, .. } = self;
        AuthServerConfig {
            socket_addr,
            secret,
            server_config,
            ipc_server_config,
            ipc_endpoint,
            rpc_middleware,
        }
    }

    /// Convenience function to start a server in one step.
    pub async fn start(self, module: AuthRpcModule) -> Result<AuthServerHandle, RpcError>
    where
        RpcMiddleware: RethRpcMiddleware,
    {
        let Self {
            socket_addr,
            secret,
            server_config,
            ipc_server_config,
            ipc_endpoint,
            rpc_middleware,
        } = self;

        // Create auth middleware.
        let middleware =
            tower::ServiceBuilder::new().layer(AuthLayer::new(JwtAuthValidator::new(secret)));

        let rpc_middleware = RpcServiceBuilder::default().layer(rpc_middleware);

        // By default, both http and ws are enabled.
        let server = ServerBuilder::new()
            .set_config(server_config.build())
            .set_http_middleware(middleware)
            .set_rpc_middleware(rpc_middleware)
            .build(socket_addr)
            .await
            .map_err(|err| RpcError::server_error(err, ServerKind::Auth(socket_addr)))?;

        let local_addr = server
            .local_addr()
            .map_err(|err| RpcError::server_error(err, ServerKind::Auth(socket_addr)))?;

        let handle = server.start(module.inner.clone());

        let ipc_handle = if let Some(ipc_server_config) = ipc_server_config {
            let ipc_endpoint_str = ipc_endpoint
                .clone()
                .unwrap_or_else(|| constants::DEFAULT_ENGINE_API_IPC_ENDPOINT.to_string());
            let ipc_server = ipc_server_config.build(ipc_endpoint_str);
            let res = ipc_server.start(module.inner).await?;
            Some(res)
        } else {
            None
        };

        Ok(AuthServerHandle { handle: Some(handle), local_addr, secret, ipc_endpoint, ipc_handle })
    }
}

/// Builder type for configuring an `AuthServerConfig`.
#[derive(Debug)]
pub struct AuthServerConfigBuilder<RpcMiddleware = Identity> {
    socket_addr: Option<SocketAddr>,
    secret: JwtSecret,
    server_config: Option<ServerConfigBuilder>,
    ipc_server_config: Option<IpcServerBuilder<Identity, Identity>>,
    ipc_endpoint: Option<String>,
    rpc_middleware: RpcMiddleware,
}

// === impl AuthServerConfigBuilder ===

impl AuthServerConfigBuilder {
    /// Create a new `AuthServerConfigBuilder` with the given `secret`.
    pub const fn new(secret: JwtSecret) -> Self {
        Self {
            socket_addr: None,
            secret,
            server_config: None,
            ipc_server_config: None,
            ipc_endpoint: None,
            rpc_middleware: Identity::new(),
        }
    }
}

impl<RpcMiddleware> AuthServerConfigBuilder<RpcMiddleware> {
    /// Configures the rpc middleware.
    pub fn with_rpc_middleware<T>(self, rpc_middleware: T) -> AuthServerConfigBuilder<T> {
        let Self { socket_addr, secret, server_config, ipc_server_config, ipc_endpoint, .. } = self;
        AuthServerConfigBuilder {
            socket_addr,
            secret,
            server_config,
            ipc_server_config,
            ipc_endpoint,
            rpc_middleware,
        }
    }

    /// Set the socket address for the server.
    pub const fn socket_addr(mut self, socket_addr: SocketAddr) -> Self {
        self.socket_addr = Some(socket_addr);
        self
    }

    /// Set the socket address for the server.
    pub const fn maybe_socket_addr(mut self, socket_addr: Option<SocketAddr>) -> Self {
        self.socket_addr = socket_addr;
        self
    }

    /// Set the secret for the server.
    pub const fn secret(mut self, secret: JwtSecret) -> Self {
        self.secret = secret;
        self
    }

    /// Configures the JSON-RPC server
    ///
    /// Note: this always configures an [`EthSubscriptionIdProvider`]
    /// [`IdProvider`](jsonrpsee::server::IdProvider) for convenience.
    pub fn with_server_config(mut self, config: ServerConfigBuilder) -> Self {
        self.server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Set the ipc endpoint for the server.
    pub fn ipc_endpoint(mut self, ipc_endpoint: String) -> Self {
        self.ipc_endpoint = Some(ipc_endpoint);
        self
    }

    /// Configures the IPC server
    ///
    /// Note: this always configures an [`EthSubscriptionIdProvider`]
    pub fn with_ipc_config(mut self, config: IpcServerBuilder<Identity, Identity>) -> Self {
        self.ipc_server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Build the `AuthServerConfig`.
    pub fn build(self) -> AuthServerConfig<RpcMiddleware> {
        AuthServerConfig {
            socket_addr: self.socket_addr.unwrap_or_else(|| {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), constants::DEFAULT_AUTH_PORT)
            }),
            secret: self.secret,
            server_config: self.server_config.unwrap_or_else(|| {
                ServerConfig::builder()
                    // This needs to large enough to handle large eth_getLogs responses and
                    // maximum payload bodies limit for
                    // `engine_getPayloadBodiesByRangeV` ~750MB per
                    // response should be enough
                    .max_response_body_size(750 * 1024 * 1024)
                    // Connections to this server are always authenticated, hence this only
                    // affects connections from the CL or any other
                    // client that uses JWT, this should be
                    // more than enough so that the CL (or multiple CL nodes) will never get
                    // rate limited
                    .max_connections(500)
                    // bump the default request size slightly, there aren't any methods exposed
                    // with dynamic request params that can exceed this
                    .max_request_body_size(128 * 1024 * 1024)
                    .set_id_provider(EthSubscriptionIdProvider::default())
            }),
            ipc_server_config: self.ipc_server_config.map(|ipc_server_config| {
                ipc_server_config
                    .max_response_body_size(750 * 1024 * 1024)
                    .max_connections(500)
                    .max_request_body_size(128 * 1024 * 1024)
                    .set_id_provider(EthSubscriptionIdProvider::default())
            }),
            ipc_endpoint: self.ipc_endpoint,
            rpc_middleware: self.rpc_middleware,
        }
    }
}

/// Holds installed modules for the auth server.
#[derive(Debug, Clone)]
pub struct AuthRpcModule {
    pub(crate) inner: RpcModule<()>,
}

impl AuthRpcModule {
    /// Create a new `AuthRpcModule` with the given `engine_api`.
    pub fn new(engine: impl IntoEngineApiRpcModule) -> Self {
        Self { inner: engine.into_rpc_module() }
    }

    /// Get a reference to the inner `RpcModule`.
    pub const fn module_mut(&mut self) -> &mut RpcModule<()> {
        &mut self.inner
    }

    /// Merge the given [Methods] in the configured authenticated methods.
    ///
    /// Fails if any of the methods in other is present already.
    pub fn merge_auth_methods(
        &mut self,
        other: impl Into<Methods>,
    ) -> Result<bool, RegisterMethodError> {
        self.module_mut().merge(other.into()).map(|_| true)
    }

    /// Removes the method with the given name from the configured authenticated methods.
    ///
    /// Returns `true` if the method was found and removed, `false` otherwise.
    pub fn remove_auth_method(&mut self, method_name: &'static str) -> bool {
        self.module_mut().remove_method(method_name).is_some()
    }

    /// Removes the given methods from the configured authenticated methods.
    pub fn remove_auth_methods(&mut self, methods: impl IntoIterator<Item = &'static str>) {
        for name in methods {
            self.remove_auth_method(name);
        }
    }

    /// Replace the given [Methods] in the configured authenticated methods.
    pub fn replace_auth_methods(
        &mut self,
        other: impl Into<Methods>,
    ) -> Result<bool, RegisterMethodError> {
        let other = other.into();
        self.remove_auth_methods(other.method_names());
        self.merge_auth_methods(other)
    }

    /// Convenience function for starting a server
    pub async fn start_server(
        self,
        config: AuthServerConfig,
    ) -> Result<AuthServerHandle, RpcError> {
        config.start(self).await
    }
}

/// A handle to the spawned auth server.
///
/// When this type is dropped or [`AuthServerHandle::stop`] has been called the server will be
/// stopped.
#[derive(Clone, Debug)]
#[must_use = "Server stops if dropped"]
pub struct AuthServerHandle {
    local_addr: SocketAddr,
    handle: Option<jsonrpsee::server::ServerHandle>,
    secret: JwtSecret,
    ipc_endpoint: Option<String>,
    ipc_handle: Option<jsonrpsee::server::ServerHandle>,
}

// === impl AuthServerHandle ===

impl AuthServerHandle {
    /// Creates a new handle that isn't connected to any server.
    ///
    /// This can be used to satisfy types that require an engine API.
    pub fn noop() -> Self {
        Self {
            local_addr: SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                constants::DEFAULT_AUTH_PORT,
            ),
            handle: None,
            secret: JwtSecret::random(),
            ipc_endpoint: None,
            ipc_handle: None,
        }
    }

    /// Returns the [`SocketAddr`] of the http server if started.
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Tell the server to stop without waiting for the server to stop.
    pub fn stop(self) -> Result<(), AlreadyStoppedError> {
        let Some(handle) = self.handle else { return Ok(()) };
        handle.stop()
    }

    /// Returns the url to the http server
    pub fn http_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Returns the url to the ws server
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.local_addr)
    }

    /// Returns a http client connected to the server.
    ///
    /// This client uses the JWT token to authenticate requests.
    pub fn http_client(&self) -> impl SubscriptionClientT + Clone + Send + Sync + Unpin + 'static {
        // Create a middleware that adds a new JWT token to every request.
        let secret_layer = AuthClientLayer::new(self.secret);
        let middleware = tower::ServiceBuilder::default().layer(secret_layer);
        jsonrpsee::http_client::HttpClientBuilder::default()
            .set_http_middleware(middleware)
            .build(self.http_url())
            .expect("Failed to create http client")
    }

    /// Returns a ws client connected to the server. Note that the connection can only be
    /// be established within 1 minute due to the JWT token expiration.
    pub async fn ws_client(&self) -> jsonrpsee::ws_client::WsClient {
        jsonrpsee::ws_client::WsClientBuilder::default()
            .set_headers(HeaderMap::from_iter([(
                AUTHORIZATION,
                secret_to_bearer_header(&self.secret),
            )]))
            .build(self.ws_url())
            .await
            .expect("Failed to create ws client")
    }

    /// Returns an ipc client connected to the server.
    #[cfg(unix)]
    pub async fn ipc_client(&self) -> Option<jsonrpsee::async_client::Client> {
        use reth_ipc::client::IpcClientBuilder;

        if let Some(ipc_endpoint) = &self.ipc_endpoint {
            return Some(
                IpcClientBuilder::default()
                    .build(ipc_endpoint)
                    .await
                    .expect("Failed to create ipc client"),
            )
        }
        None
    }

    /// Returns an ipc handle
    pub fn ipc_handle(&self) -> Option<jsonrpsee::server::ServerHandle> {
        self.ipc_handle.clone()
    }

    /// Return an ipc endpoint
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
    }
}
