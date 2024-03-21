E2eToolchainInfo = provider(fields = {
    "e2e_test": typing.Any,
})

def e2e_toolchain_impl(ctx) -> list[[DefaultInfo, E2eToolchainInfo]]:
    """
    A e2e test execution toolchain.
    """
    return [
        DefaultInfo(),
        E2eToolchainInfo(
            e2e_test = ctx.attrs._rootfs_build,
        ),
    ]

e2e_toolchain = rule(
    impl = e2e_toolchain_impl,
    attrs = {
        "_e2e_test": attrs.dep(
            default = "prelude-si//e2e:e2e_test.sh",
        ),
    },
    is_toolchain_rule = True,
) 