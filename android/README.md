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

- Tabs, the task tree (fold with the arrows, remembered), done boxes, a ▶/■
  timer per task. Search (all tabs), "What's next?", time spent (today,
  this week… per tab and task).
- Task screen: title, notes, priority, due date (`fri 9:00`, `17:30`…),
  tags, subtasks, requirements; menu: indent / outdent, move under another
  task, move to another tab (with subtasks and work log), delete (with undo).
  Work log: tap a session to correct or delete it, "Log time" to add one.
- Settings: your name and email, this phone's keys, joining a shared tab;
  for the current tab: name, sharing, inviting and removing members, moving
  it to another remote, stopping its sync, default priority, deleting it.
- Sync on start, a few seconds after edits, and every 15 minutes in the
  background. Due notifications on time, with "Done" and "In 1 hour".

## Remotes

`rsync:user@host:path` remotes are reached over SFTP (sshj), with the app's
own SSH key (Settings shows it: add it to the server's `authorized_keys`).
The host key is trusted the first time, then checked. A key restricted with
`rrsync` does not allow SFTP: use e.g.
`command="internal-sftp -d /path",restrict` for the phone's key instead.
A port other than 22: `rsync:user@host:2222:path`.

Directories (e.g. one synced by Syncthing) work too.
