# Filterway

is a lightweight Wayland socket proxy that intercepts messages and filters/modifies them. You can use it for, for example, making all applications in a container have the same `app_id` (apply certain window decorations in certain containers).

Current features:

- Force `app_id` - assign all toplevels the same `app_id` and suppress client-originated `set_app_id` requests
- Dump wayland protocol traffic

# How to use it

Your main compositor will have created something like `/run/user/1000/wayland-0` where `1000` is your user ID.

1. Build `filterway` with `cargo build`.

   Note, socket ancillary data (required by Wayland protocol) requires unstable rust currently. If you use `rustup` to manage rust it should read the `rust-toolchain.toml` file and compile accordingly.

2. Run `filterway /run/user/1000/wayland-0 /run/user/1000/wayland-filtered org.example.testid`

   Run `filterway --help` for details.

3. Run Wayland applications or another compositor with `WAYLAND_DISPLAY=wayland-filtered`
