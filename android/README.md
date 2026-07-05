# messages web viewer (Android)

The `messages.xinutec.org` archive viewer presented as a native-feeling app: a
single full-screen **WebView**, no address bar, no tabs, a home-screen icon. It
avoids browser chrome while showing the UI exactly as designed (the system WebView
is Chromium, so it renders like Chrome).

The site is **private** — reachable over the VPN — and **behind a login**. The
WebView keeps the session cookie, so it's a **one-time sign-in**; the app needs
only `INTERNET` (the VPN is set up at the OS/network level, not by this app).

## What it does

- Loads `https://messages.xinutec.org/` — **hardcoded** (`MainActivity.MESSAGES_URL`);
  this app is single-purpose.
- JavaScript + DOM storage on (Angular), all navigation kept in-app, Back walks the
  SPA history; reopens on the last in-app page.
- Insets the WebView from the system bars by padding a wrapper, and paints the
  strips behind the bars with the page's own surface colour (read on load, so it
  tracks the Material light/dark theme).

Runs on any Android 8+ (minSdk 26) device. Must be on the VPN to reach the host.

## Build & install

No toolchain lives in this repo — it borrows the recall project's `android` nix
dev shell (JDK 17 + Android SDK; the Gradle wrapper pins Gradle):

```sh
cd android
nix develop ~/Code/recall#android --command ./gradlew :app:assembleDebug
# → app/build/outputs/apk/debug/app-debug.apk
```

Install onto a phone over WiFi (Pixel 9 is at `192.168.1.133:5555`):

```sh
ADB="$ANDROID_HOME/platform-tools/adb"   # inside the nix shell above
"$ADB" connect 192.168.1.133:5555
"$ADB" -s 192.168.1.133:5555 install -r app/build/outputs/apk/debug/app-debug.apk
"$ADB" -s 192.168.1.133:5555 shell am start -n org.xinutec.messages/.MainActivity
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
│       ├── kotlin/org/xinutec/messages/MainActivity.kt  # the WebView (+ inset padding)
│       └── res/                                  # launcher icon (blue chat bubble), theme, strings
├── build.gradle.kts · settings.gradle.kts · gradle/   # project scaffolding
└── gradlew                                       # borrows ~/Code/recall#android for the SDK
```
