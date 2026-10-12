# Startup and CPU overrides for the MEV stack in the pinned Ethereum package.
def relay_startup_plan(plan):
    # The upstream housekeeper has no ports/readiness check, so add_service
    # returns while it is still migrating Postgres. The API then races those
    # migrations. In the pinned relay, /metrics opens only after DB setup and
    # beacon readiness succeed. Probe inside the container: its default listener
    # is loopback-only. A later set_service readiness override does not affect
    # the original add_service, so insert the wait in the upstream launch path.
    def add_service(name, config, **kwargs):
        service = plan.add_service(name=name, config=config, **kwargs)
        if name == "mev-relay-housekeeper":
            plan.wait(
                service_name=name,
                recipe=ExecRecipe(command=[
                    "wget", "-q", "-T", "2", "-O", "/dev/null",
                    "http://127.0.0.1:9064/metrics",
                ]),
                field="code",
                assertion="==",
                target_value=0,
                interval="1s",
                timeout="120s",
                description="wait for relay housekeeper database migrations",
            )
        return service

    methods = {name: getattr(plan, name) for name in dir(plan)}
    methods["add_service"] = add_service
    return struct(**methods)


# Other profiles retain upstream defaults when mev_relay_cpu is absent.
def configure_mev_relay_cpu(plan, eth_args, limits):
    if limits == None:
        return
    if type(limits) != "dict" or sorted(limits.keys()) != ["max", "min"]:
        fail("eez.mev_relay_cpu must contain min and max millicpus")
    min_cpu = limits["min"]
    max_cpu = limits["max"]
    if type(min_cpu) != "int" or type(max_cpu) != "int" or min_cpu < 10 or max_cpu < min_cpu:
        fail("eez.mev_relay_cpu requires integers with 10 <= min <= max")
    if eth_args.get("mev_type") != "flashbots":
        fail("eez.mev_relay_cpu requires ethereum_package.mev_type: flashbots")
    relay_image = eth_args.get("mev_params", {}).get("mev_relay_image", "")
    if not relay_image:
        fail("set ethereum_package.mev_params.mev_relay_image explicitly with mev_relay_cpu")

    # Kurtosis 1.20 requires an image even for a resource-only set_service.
    # Reuse the selected relay image and the Redis package's existing image.
    # Keep these service names/images aligned with the Ethereum package pin.
    images = {
        "mev-relay-redis": "redis:alpine",
        "mev-relay-api": relay_image,
        "mev-relay-housekeeper": relay_image,
        "mev-relay-website": relay_image,
    }
    for name, image in images.items():
        plan.set_service(
            name=name,
            config=ServiceConfig(image=image, min_cpu=min_cpu, max_cpu=max_cpu),
        )
