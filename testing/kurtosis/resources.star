# CPU-only overrides for the MEV stack in the pinned Ethereum package.
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
