# cpasfait for Android

Kotlin + Jetpack Compose over the Rust core (`crates/cpasfait-ffi`). The
Gradle build compiles the core with `cargo ndk` and generates its Kotlin
bindings, so it needs, besides the Android SDK: the NDK (SDK Manager → SDK
Tools → NDK), `rustup target add aarch64-linux-android x86_64-linux-android`
and `cargo install cargo-ndk`.

```sh
cd android
./gradlew assembleDebug        # app/build/outputs/apk/debug/app-debug.apk
./gradlew installDebug         # on the connected phone / running emulator
```

Or open this directory in Android Studio.

## What it does

- Tabs, the task tree (fold with the arrows), done boxes, a ▶/■ timer per task.
- Task screen: title, notes, priority, due date (`fri 9:00`, `17:30`…),
  tags, subtasks, the work log.
- Settings: your name and email, this phone's keys, joining a shared tab,
  sharing / inviting.
- Sync on start, a few seconds after edits, and every 15 minutes in the
  background; notifications when tasks fall due.

## Remotes

`rsync:user@host:path` remotes are reached over SFTP (sshj), with the app's
own SSH key (Settings shows it: add it to the server's `authorized_keys`).
The host key is trusted the first time, then checked. A key restricted with
`rrsync` does not allow SFTP: use e.g.
`command="internal-sftp -d /path",restrict` for the phone's key instead.
A port other than 22: `rsync:user@host:2222:path`.

Directories (e.g. one synced by Syncthing) work too.
