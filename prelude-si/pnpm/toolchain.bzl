PnpmToolchainInfo = provider(fields = [
    "build_npm_bin",
    "build_package_node_modules",
    "build_pkg_bin",
    "build_typescript_runnable_dist_bin",
    "build_workspace_node_modules",
    "exec_cmd",
    "package_build_context",
    "package_dist_context",
    "run_pnpm_script",
])

def pnpm_toolchain_impl(ctx) -> list[[DefaultInfo, PnpmToolchainInfo]]:
    """
    A Pnpm toolchain.
    """
    return [
        DefaultInfo(),
        PnpmToolchainInfo(
            build_npm_bin = ctx.attrs._build_npm_bin,
            build_package_node_modules = ctx.attrs._build_package_node_modules,
            build_pkg_bin = ctx.attrs._build_pkg_bin,
            build_workspace_node_modules = ctx.attrs._build_workspace_node_modules,
            build_typescript_runnable_dist_bin = ctx.attrs._build_typescript_runnable_dist_bin,
            exec_cmd = ctx.attrs._exec_cmd,
            package_build_context = ctx.attrs._package_build_context,
            package_dist_context = ctx.attrs._package_dist_context,
            run_pnpm_script = ctx.attrs._run_pnpm_script,
        )
    ]

pnpm_toolchain = rule(
    impl = pnpm_toolchain_impl,
    attrs = {
        "_build_npm_bin": attrs.dep(
            default = "prelude-si//pnpm:build_npm_bin.py",
        ),
        "_build_package_node_modules": attrs.dep(
            default = "prelude-si//pnpm:build_package_node_modules.py",
        ),
        "_build_pkg_bin": attrs.dep(
            default = "prelude-si//pnpm:build_pkg_bin.py",
        ),
        "_build_typescript_runnable_dist_bin": attrs.dep(
            default = "prelude-si//pnpm:build_typescript_runnable_dist_bin.py",
        ),
        "_build_workspace_node_modules": attrs.dep(
            default = "prelude-si//pnpm:build_workspace_node_modules.py",
        ),
        "_exec_cmd": attrs.dep(
            default = "prelude-si//pnpm:exec_cmd.py",
        ),
        "_package_build_context": attrs.dep(
            default = "prelude-si//pnpm:package_build_context.py",
        ),
        "_package_dist_context": attrs.dep(
            default = "prelude-si//pnpm:package_dist_context.py",
        ),
        "_run_pnpm_script": attrs.dep(
            default = "prelude-si//pnpm:run_pnpm_script.py",
        ),
    },
    is_toolchain_rule = True,
)
