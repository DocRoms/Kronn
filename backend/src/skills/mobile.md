---
name: Mobile
description: React Native, Flutter, offline-first, push notifications, and mobile best practices
category: domain
icon: 📱
builtin: true
---

Mobile development expertise covering cross-platform and native patterns:

- Cross-platform: React Native for JavaScript teams, Flutter for performance-critical UIs. Evaluate native when platform-specific features dominate.
- Offline-first: design for no network. Local database (SQLite, Realm, Hive) as source of truth. Sync when connectivity returns. Handle conflicts.
- State management: keep UI state and server state separate. Use optimistic updates for responsiveness. Roll back on sync failure.
- Push notifications: use FCM (Android) and APNs (iOS). Handle foreground, background, and killed states differently. Never spam — let users control preferences.
- Navigation: stack-based navigation is the norm. Deep linking from day one — retrofit is painful. Handle back button correctly on Android.
- Performance: avoid unnecessary re-renders. Use lazy loading for lists (FlatList, ListView.builder). Profile with platform tools (Flipper, DevTools).
- Responsive layouts: design for multiple screen sizes. Use relative units, not fixed pixels. Test on small phones and tablets.
- Native bridges: minimize bridge crossings (React Native) or platform channels (Flutter). Batch operations. Heavy computation on the native side.
- App store guidelines: follow Apple HIG and Material Design. Handle review rejection gracefully. No hidden functionality. Respect privacy APIs.
- Security: store secrets in Keychain/Keystore, not SharedPreferences. Certificate pinning for sensitive APIs. Obfuscate release builds.

When reviewing mobile code, flag: synchronous network calls on main thread, missing offline handling, hardcoded dimensions, excessive bridge calls, and missing permission handling.
