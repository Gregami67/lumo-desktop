# Arch Linux and Artix Linux packaging

This directory contains a source `PKGBUILD` for Lumo Desktop. It works on
Arch Linux and on Artix Linux with any of its init systems. If you would rather
install Proton's official binary, the AUR has `lumo-desktop-bin`; this package
is the from-source equivalent and conflicts with it.

## Dependencies

The Linux build needs **WebKitGTK 4.1**, not WebKitGTK 4.0. On Arch and Artix
that package is called `webkit2gtk-4.1`; `webkit2gtk` is the older 4.0 build and
will fail the build. The `PKGBUILD` declares `webkit2gtk-4.1` for you.

Runtime dependencies are declared in the `PKGBUILD`. A keyring daemon
(`gnome-keyring` or `kwallet`) is recommended for connector credentials.

## Building

From this directory:

```bash
# Build and install the pinned upstream commit (what an AUR package would use)
makepkg -si

# Build and install this checkout, including local changes
./makepkg-local.sh -si
```

`makepkg-local.sh` sets `LUMO_PKGBUILD_LOCAL=1`, which makes the `PKGBUILD`
build the repository it lives in and puts Cargo output under `src/target/`
instead of touching `src-tauri/target/`. The build also sets
`LUMO_URL=https://lumo.proton.me`, so the installed app uses the production
Lumo web app rather than the development URL.

The `PKGBUILD` uses `options=('!lto')`. Makepkg's global `lto` option adds
`-flto` to CFLAGS/CXXFLAGS; `aws-lc-sys` then emits GCC LTO objects that Rust's
lld link step cannot read, and the build fails with undefined `aws_lc_*`
symbols. The Rust release profile already enables LTO.

## Starting at login

Lumo has a **Launch at startup** setting. It uses XDG autostart, which works
with systemd, OpenRC, runit, s6, dinit and every desktop environment that
implements the XDG autostart specification. This is the recommended option.

The package also ships init-specific user services for people who want their
init system to supervise the app:

### systemd

```bash
systemctl --user daemon-reload
systemctl --user enable --now lumo-desktop.service
```

The unit is `lumo-desktop.service`, installed in `/usr/lib/systemd/user/` and
bound to `graphical-session.target`.

### OpenRC (Artix)

```bash
rc-update --user add lumo-desktop default
rc-service --user lumo-desktop start
```

The script is installed in `/etc/user/init.d/lumo-desktop`, with defaults in
`/etc/user/conf.d/lumo-desktop`. OpenRC user services need a user OpenRC
session (Artix's `openrc-user` PAM setup) and the Artix `dbus-openrc` package
(plus `elogind-openrc` if you use elogind). If you do not have that, use the
in-app XDG autostart setting instead.

### runit, s6 and dinit

These Artix variants have no user-service equivalent in this package. Use the
in-app **Launch at startup** setting, which is init-independent, or adapt the
systemd unit to your supervisor.

## Updating the package

1. Update `pkgver` and `_commit` in `PKGBUILD`.
2. Regenerate `.SRCINFO`:

   ```bash
   makepkg --printsrcinfo > .SRCINFO
   ```

3. Rebuild and test with `./makepkg-local.sh -si`.

The built-in Tauri updater is not used for pacman installs; update through
`pacman`/your AUR helper instead.
