# Filterway

is a Wayland socket proxy that can rewrite `app_id` for any programs that use it's downstream socket. This allows you to write compositor decoration rules and other things for a set of programs based on the modified `app_id`.

This can both replace and prefix the `app_id`.

# How to use it

Your main compositor will have created something like `/run/user/1000/wayland-0` where `1000` is your user ID.

1. Build `filterway` with `cargo build`.

   Note, socket ancillary data (required by Wayland protocol) requires unstable rust currently. If you use `rustup` to manage rust it should read the `rust-toolchain.toml` file and compile accordingly.

2. Run `filterway --upstream /run/user/1000/wayland-0 --downstream /run/user/1000/wayland-filtered --app-id org.example.testid`

   Run `filterway --help` for details.

3. Run Wayland applications or another compositor with `WAYLAND_DISPLAY=wayland-filtered`
