# cpasfait-ffi

The core for the mobile apps: a `Cpasfait` object (one database connection)
and plain records, exported to Kotlin with [UniFFI](https://mozilla.github.io/uniffi-rs).
See `src/lib.rs` for the API, and `android/` for the app using it.

## Kotlin bindings

```sh
cargo build -p cpasfait-ffi
cargo run -p cpasfait-ffi --features cli --bin uniffi-bindgen -- \
    generate --library target/debug/libcpasfait_ffi.so --language kotlin --out-dir OUT
```

`OUT/cpasfait/core/cpasfait_ffi.kt` uses [JNA](https://github.com/java-native-access/jna)
(`net.java.dev.jna:jna:…@aar` on Android) to load `libcpasfait_ffi.so`.

## Android libraries

With the NDK installed (Android SDK manager) and
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk):

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
cargo ndk -t arm64-v8a -t x86_64 -o app/src/main/jniLibs build --release -p cpasfait-ffi
```

(`arm64-v8a` for phones, `x86_64` for the emulator.)

## Using it

- `Cpasfait.open(context.filesDir.path + "/cpasfait", ZoneId.systemDefault().id)`
  once per connection; one data directory per process.
- Sync is slow (network): run `sync` on a second instance, off the main
  thread (WorkManager), and reload the screens when `dataVersion()` of the UI
  instance changes.
- Remotes: directories work as they are. Implement `RemoteProvider` to serve
  others, e.g. `rsync:user@host:path` over SFTP (sshj): the same files as
  the desktops' rsync, under the same spec, so moves and invitations are the
  same on every device. `putNew` must not overwrite: write a temporary file,
  then rename it (SFTP's plain rename fails if the target exists).
- Due dates: `upcomingDues()` to schedule alarms, `takeDueNotices()` when one
  fires (each due date is announced once, across the apps using that
  database).
