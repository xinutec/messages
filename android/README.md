# messages web viewer (Android)

The `messages.xinutec.org` archive viewer presented as a native-feeling app: a
single full-screen WebView, no address bar, no tabs, a home-screen icon. It
avoids browser chrome while showing the UI exactly as designed (the system WebView
is Chromium, so it renders like Chrome).

The site is private (VPN only) and behind a login. The WebView keeps the session
cookie, so sign-in is once; the app needs
only `INTERNET` (the VPN is set up at the OS/network level, not by this app).

## What it does

The WebView itself (in-app navigation, Back through the SPA history, reopening
on the last page, insets and system-bar colour from the page's theme) is the
fleet's shared `WebShellActivity`, in `ui-harness/android`, which must be checked
out beside this repo. `MainActivity` adds what is this app's own:

- the URL, `https://messages.xinutec.org/` (`MainActivity.MESSAGES_URL`), and the
  Nextcloud host the login passes through; other links open in the browser;
- a way up out of a conversation reached by a cold launch, which has no in-app
  history for Back to walk.

Runs on any Android 8+ (minSdk 26) device. Must be on the VPN to reach the host.

## Build & install

No toolchain lives in this repo — it borrows the recall project's `android` nix
dev shell (JDK 17 + Android SDK; the Gradle wrapper pins Gradle):

```sh
cd android
nix develop ~/Code/recall#android --command ./gradlew :app:assembleDebug
# → app/build/outputs/apk/debug/app-debug.apk
```

Install onto the Pixel 9 with `deploy.sh`, which connects over the VPN or LAN,
checks the device model, and installs by serial:

```sh
nix develop ~/Code/recall#android --command ./deploy.sh [<ip[:port]>]
```

The APK is signed with the auto-generated debug key — fine for sideloading, the
only distribution path.

## Layout

```
android/
├── app/
│   ├── build.gradle.kts                          # android app module, no Compose/AppCompat
│   └── src/main/
│       ├── AndroidManifest.xml                   # INTERNET; single launcher activity
│       ├── kotlin/org/xinutec/messages/MainActivity.kt  # URL, allowed hosts, up out of a thread
│       └── res/                                  # launcher icon (blue chat bubble), theme, strings
├── build.gradle.kts · settings.gradle.kts · gradle/   # project scaffolding
└── gradlew                                       # borrows ~/Code/recall#android for the SDK
```
