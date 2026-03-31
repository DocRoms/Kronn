---
name: mobile
description: Use when building or reviewing React Native, Flutter, or native iOS/Android code. Covers offline-first, push notifications, navigation, and cross-platform patterns.
license: AGPL-3.0
category: domain
icon: 📱
builtin: true
---

## Procedure

1. **Design offline-first**: local DB (SQLite, Realm, Hive) as source of truth. Sync on reconnect. Handle conflicts explicitly.
2. **Set up navigation**: stack-based. Implement deep linking from day one — retrofitting is painful. Handle Android back button.
3. **Virtualize lists**: use `FlatList`/`ListView.builder`. Never render unbounded items in a `ScrollView`.
4. **Minimize bridge crossings**: batch native calls (RN bridge / Flutter platform channels). Move heavy computation to native side.
5. **Handle push correctly**: FCM (Android) + APNs (iOS). Implement foreground, background, AND killed-state handlers separately.
6. **Secure secrets**: Keychain (iOS) / Keystore (Android). Never `SharedPreferences` for tokens or keys. Certificate pinning for sensitive APIs.

## Gotchas

- React Native's `bridge` serializes JSON on every crossing. Passing large arrays kills performance — flatten or paginate.
- `FlatList` with missing `keyExtractor` silently re-renders entire list on every state change.
- Android `FLAG_SECURE` prevents screenshots but also breaks accessibility tools — test on both paths.
- iOS rejects apps that request permissions without a clear usage string. Add `NSxxxUsageDescription` BEFORE the review, not after rejection.
- `expo-updates` OTA bypasses app store review — Apple allows it for JS-only changes but rejects native-code OTA.
- Hardcoded pixel dimensions break on foldables and tablets. Use relative units (`%`, `flex`, `Dimensions`).

## Validation

Profile with Flipper (RN) or DevTools (Flutter). Test on a low-end device. Verify offline flow: enable airplane mode, use the app, reconnect, confirm sync.

✓ `FlatList` with `keyExtractor`, pagination, and offline cache fallback
✗ 1000 items in `ScrollView`, no virtualization, crashes on low-end devices
