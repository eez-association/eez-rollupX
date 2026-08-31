# Chain-specific Blobscan deployment for private EEZ Kurtosis networks.

POSTGRES_PORT = 5432
REDIS_PORT = 6379
API_PORT = 3001
WEB_PORT = 3000
SECRET_KEY = "eez-disposable-devnet-blobscan-secret"


def launch(plan, chain_id, execution_rpc_url, beacon_rpc_url, params):
    postgres_service = "blobscan-postgres"
    redis_service = "blobscan-redis"
    api_service = "blobscan-api"

    postgres_user = "blobscan"
    postgres_password = "blobscan"
    postgres_database = "blobscan"
    postgres_url = "postgresql://{}:{}@{}:{}/{}".format(
        postgres_user,
        postgres_password,
        postgres_service,
        POSTGRES_PORT,
        postgres_database,
    )
    redis_url = "redis://{}:{}".format(redis_service, REDIS_PORT)

    plan.add_service(
        name=postgres_service,
        config=ServiceConfig(
            image=params["postgres_image"],
            ports={
                "postgres": PortSpec(
                    number=POSTGRES_PORT,
                    transport_protocol="TCP",
                    wait="2m",
                ),
            },
            env_vars={
                "POSTGRES_USER": postgres_user,
                "POSTGRES_PASSWORD": postgres_password,
                "POSTGRES_DB": postgres_database,
            },
        ),
    )

    plan.add_service(
        name=redis_service,
        config=ServiceConfig(
            image=params["redis_image"],
            ports={
                "redis": PortSpec(
                    number=REDIS_PORT,
                    transport_protocol="TCP",
                    wait="2m",
                ),
            },
        ),
    )

    plan.add_service(
        name=api_service,
        config=ServiceConfig(
            image=params["api_image"],
            ports={
                "http": PortSpec(
                    number=API_PORT,
                    transport_protocol="TCP",
                    application_protocol="http",
                ),
            },
            env_vars={
                "BLOBSCAN_API_PORT": str(API_PORT),
                "CHAIN_ID": str(chain_id),
                "DATABASE_URL": postgres_url,
                "DENCUN_FORK_SLOT": "0",
                "DIRECT_URL": postgres_url,
                "ETH_PRICE_SYNCER_ENABLED": "false",
                "NETWORK_NAME": "devnet",
                "POSTGRES_STORAGE_ENABLED": "true",
                "REDIS_URI": redis_url,
                "SECRET_KEY": SECRET_KEY,
            },
            ready_conditions=ReadyCondition(
                recipe=GetHttpRequestRecipe(
                    port_id="http",
                    endpoint="/healthcheck",
                ),
                field="code",
                assertion="==",
                target_value=200,
                interval="5s",
                timeout="300s",
            ),
        ),
    )

    plan.add_service(
        name="blobscan-web",
        config=ServiceConfig(
            image=params["web_image"],
            ports={
                "http": PortSpec(
                    number=WEB_PORT,
                    transport_protocol="TCP",
                    application_protocol="http",
                ),
            },
            env_vars={
                "BLOBSCAN_API_BASE_URL": "http://{}:{}".format(
                    api_service,
                    API_PORT,
                ),
                "CHAIN_ID": str(chain_id),
                "DATABASE_URL": postgres_url,
                "DIRECT_URL": postgres_url,
                "POSTGRES_STORAGE_ENABLED": "true",
                "PUBLIC_NETWORK_NAME": "devnet",
                "REDIS_URI": redis_url,
                "SECRET_KEY": SECRET_KEY,
            },
            ready_conditions=ReadyCondition(
                recipe=GetHttpRequestRecipe(
                    port_id="http",
                    endpoint="/",
                ),
                field="code",
                assertion="==",
                target_value=200,
                interval="5s",
                timeout="300s",
            ),
        ),
    )

    plan.add_service(
        name="blobscan-indexer",
        config=ServiceConfig(
            image=params["indexer_image"],
            env_vars={
                "BEACON_NODE_ENDPOINT": beacon_rpc_url,
                "BLOBSCAN_API_ENDPOINT": "http://{}:{}".format(
                    api_service,
                    API_PORT,
                ),
                "DENCUN_FORK_SLOT": "0",
                "EXECUTION_NODE_ENDPOINT": execution_rpc_url,
                "NETWORK_NAME": "devnet",
                "RUST_LOG": params.get("indexer_rust_log", "info"),
                "SECRET_KEY": SECRET_KEY,
            },
            entrypoint=["/app/blob-indexer"],
            cmd=["--from-slot", "0"],
        ),
    )
