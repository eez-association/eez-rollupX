-- Filter metadata contains no credentials, endpoints, private directory paths,
-- or raw quarantine data. Grafana does not need access to administrative tables.
CREATE VIEW eez.dashboard_chain_context AS
 SELECT chain.id AS chain_instance_id,deployment.id AS deployment_id,
   deployment.environment,deployment.name AS deployment,chain.role,
   chain.chain_id,chain.genesis_hash,chain.rollup_id
 FROM eez.chain_instances chain JOIN eez.deployments deployment ON deployment.id=chain.deployment_id;
GRANT SELECT ON eez.dashboard_chain_context TO eez_observer_writer,eez_observer_reader;

-- Reader-side dashboards share the observer's finite query/lock budgets. Scope
-- settings to this database; fresh pooled connections pick them up on login.
DO $$ BEGIN
 EXECUTE format('ALTER ROLE eez_observer_reader IN DATABASE %I SET statement_timeout = %L',current_database(),'10s');
 EXECUTE format('ALTER ROLE eez_observer_reader IN DATABASE %I SET lock_timeout = %L',current_database(),'5s');
 EXECUTE format('ALTER ROLE eez_observer_reader IN DATABASE %I SET idle_in_transaction_session_timeout = %L',current_database(),'10s');
END $$;
