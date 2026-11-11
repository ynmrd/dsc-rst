# Discord Native — Complete Electron-to-Rust Rewrite

## Your Role

Act as a **Senior Rust Engineer, Desktop Application Architect, Real-Time Communication Engineer, WebRTC Specialist, Performance Optimization Engineer, and UI/UX Developer**.

Your mission is to rebuild the Discord desktop client as a **fully native, highly optimized Rust application**, replacing Electron entirely while preserving as much of the original Discord functionality, user experience, and cross-client compatibility as technically and legally possible.

This is not a prototype, mockup, browser wrapper, or simplified chat application.

**The ultimate goal is a real, production-quality Discord-compatible desktop client that can serve as a daily replacement for the official Electron application.**

Do not simply recreate Discord's interface. Implement actual functionality.

---

## 1. Primary Objectives

Build a Discord desktop client that:

- Uses **Rust as its primary implementation language**.
- Completely eliminates Electron, Chromium, Node.js, and unnecessary web-based runtime dependencies.
- Uses a native desktop UI rendering framework.
- Significantly reduces RAM and CPU consumption.
- Provides fast startup, smooth navigation, and responsive interactions.
- Supports authentication with an existing Discord account through authorized mechanisms.
- Communicates with the existing Discord infrastructure wherever supported.
- Allows interaction with users who use the official Discord desktop, web, and mobile clients.
- Supports voice communication, messaging, servers, channels, and other core Discord functionality wherever technically supported.
- Maintains visual and functional parity with the official client as closely as possible.

**Do not build a separate Discord-like platform. The objective is compatibility with the existing Discord ecosystem, not the creation of another messaging service.**

Research Discord's current APIs, SDKs, developer policies, and protocol restrictions before implementation. Never assume that bot APIs or OAuth2 automatically provide full user-client functionality.

If Discord does not officially support a required capability for third-party clients, document the limitation accurately. Do not bypass authentication protections, harvest user tokens, impersonate the official client, or falsely claim that unsupported features work.

---

## 2. Technology Stack

Use the following preferred architecture, modifying individual components only when benchmarks or technical requirements justify alternatives.

| Component | Preferred technology |
|---|---|
| Core application | Rust |
| Desktop UI | Slint, iced, or another performant native Rust UI framework |
| Graphics rendering | wgpu |
| Async runtime | Tokio |
| HTTP networking | reqwest |
| WebSocket communication | tokio-tungstenite |
| Real-time audio | CPAL |
| Audio codec | Opus |
| Real-time communication | WebRTC-compatible Rust implementation |
| Local database | SQLite |
| Serialization | serde |
| Logging and diagnostics | tracing |
| Secure credential storage | OS credential manager |
| Primary platform | Windows 10/11 |

Evaluate each library before choosing it. Prefer stable, actively maintained projects.

**Do not use Electron, Tauri, embedded Chromium, or a WebView-based architecture as the primary application UI.**

Rust must control the application lifecycle, networking, state management, rendering integration, and multimedia systems.

---

## 3. Discord Account Integration

Implement a secure authentication and account-management system.

Required capabilities, where supported:

- Sign in with an existing Discord account.
- Maintain authenticated sessions securely.
- Restore a valid session after restarting the application.
- Support account switching.
- Display the user's profile, avatar, username, and status.
- Load accessible servers, channels, and conversations.
- Handle session expiration and authentication failures.
- Support logout and credential cleanup.

Use only supported authentication methods with the necessary permissions.

Never ask users to extract Discord tokens from browser storage, developer tools, or the official client.

Do not implement password interception, CAPTCHA bypasses, or mechanisms intended to circumvent Discord's security controls.

---

## 4. Messaging System

Implement a complete real-time messaging interface to the extent the available authorized APIs permit.

### Core features

- Direct messages.
- Group conversations.
- Server text channels.
- Message sending and receiving.
- Message editing and deletion.
- Replies and mentions.
- Message reactions.
- Custom emoji.
- GIFs and stickers.
- File uploads and downloads.
- Image previews.
- Embedded content.
- Message history.
- Search.
- Pinned messages.
- Unread indicators.
- Typing indicators.
- Message notifications.
- Thread support.
- Message formatting and Markdown rendering.

### Performance

Use message virtualization to render large conversations efficiently.

Implement incremental loading, asynchronous image decoding, lazy attachment loading, and a memory-bounded cache.

Scrolling through thousands of messages must remain responsive.

---

## 5. Voice Chat — Critical Priority

Voice chat is one of the most important requirements.

The client must be designed to communicate with users running the official Discord client, subject to Discord's supported integration capabilities.

Research and implement the supported real-time communication stack.

### Required features

- Joining and leaving voice channels.
- Two-way voice communication.
- Microphone selection.
- Speaker/output-device selection.
- Microphone mute.
- Audio deafen.
- Individual user volume controls.
- Voice activity detection.
- Push-to-talk.
- Input sensitivity adjustment.
- Audio device hot switching.
- Automatic reconnection.
- Network interruption recovery.
- Speaking indicators.
- Low-latency audio processing.
- Echo cancellation where supported.
- Noise suppression where supported.
- Automatic gain control where appropriate.

### Audio quality

Prioritize:

- Low latency.
- Clear audio.
- Stable real-time playback.
- Minimal packet loss disruption.
- Low CPU usage.
- Proper jitter buffering.
- Efficient audio encoding and decoding.

Investigate Opus and WebRTC-compatible implementations.

Test how the audio system behaves during network instability, device disconnection, and simultaneous multiple-user conversations.

**Do not report voice-chat compatibility as implemented until real interoperability testing demonstrates successful bidirectional communication with an official Discord client.**

---

## 6. Screen Sharing and Video

Implement supported multimedia capabilities:

- Screen capture.
- Application-window capture.
- Screen sharing.
- Screen-share viewing.
- Webcam capture.
- Webcam streaming.
- Video playback.
- Stream audio when supported.
- Fullscreen viewing.
- Resolution selection.
- Frame-rate selection.
- Hardware-accelerated encoding and decoding where available.

Prefer native Windows capture APIs, efficient GPU texture handling, and minimized CPU-GPU memory transfers.

Avoid unnecessary video-frame copies.

Investigate support for NVIDIA, AMD, and Intel hardware acceleration.

If interoperability with Discord's existing media infrastructure is not supported by its available APIs, document the blocker rather than implementing a misleading local-only substitute.

---

## 7. Native UI and Visual Design

Recreate the familiar Discord desktop experience using native rendering.

The application should feel visually consistent with Discord while avoiding unauthorized reuse of proprietary source code or assets.

Implement:

- Server sidebar.
- Channel list.
- Chat interface.
- Member list.
- User profile panel.
- Voice-channel interface.
- Friends list.
- Direct-message navigation.
- Search interface.
- Settings panel.
- Notification interface.
- Context menus.
- Tooltips.
- Modal dialogs.
- Drag-and-drop interactions.
- Keyboard shortcuts.
- Dark and light themes.
- Smooth transitions and animations.

### UI performance requirements

Target 60 FPS or higher for typical interactions.

Avoid blocking the UI thread with network requests, database operations, image decoding, or media processing.

Use reactive state updates and minimize unnecessary rerendering.

The interface must remain responsive while messages arrive, audio is processed, and large media files are transferred.

---

## 8. Advanced Discord Features

Investigate and implement additional functionality wherever supported:

- Server folders.
- User presence.
- Custom statuses.
- Rich Presence integration.
- User profiles.
- Server roles.
- Channel permissions.
- Server management.
- Notifications and mentions.
- Server discovery.
- Forum channels.
- Scheduled events.
- Polls.
- Activity panels.
- Keyboard shortcuts.
- System tray integration.
- Desktop notifications.
- Automatic startup.
- Application updates.
- Game activity detection.
- Multiple accounts.

Clearly distinguish between fully implemented features, partially supported features, unsupported third-party integrations, and features blocked by official API limitations.

---

## 9. Performance Optimization

The primary reason for this rewrite is to achieve significantly better resource efficiency than the Electron-based client.

Optimize the entire application rather than only replacing the UI framework.

### Memory optimization

- Avoid unnecessary memory allocations.
- Use bounded caches.
- Reuse buffers.
- Reduce unnecessary data duplication.
- Load assets lazily.
- Free resources when navigating away from views.
- Prevent memory leaks.
- Optimize message storage.
- Avoid retaining unnecessary historical state.

### CPU optimization

- Use asynchronous I/O.
- Avoid inefficient polling.
- Minimize lock contention.
- Optimize message parsing.
- Batch UI state updates.
- Reduce redundant computations.
- Use efficient event-driven architecture.

### GPU optimization

- Minimize rendering overhead.
- Avoid unnecessary redraws.
- Cache reusable graphical resources.
- Optimize image and texture memory.
- Support efficient video processing.
- Measure GPU usage during idle and active workloads.

### Benchmark targets

Treat these as initial engineering targets, not guaranteed results.

| Metric | Initial target |
|---|---|
| Idle RAM usage | Below 150 MB where feasible |
| Idle CPU usage | Near 0% |
| Cold startup | Under 2 seconds on a modern SSD |
| UI responsiveness | 60+ FPS |
| Message scrolling | Smooth with large histories |
| Voice communication | Low latency and stable |
| Memory leaks | None detectable during extended tests |

Compare results against the official Discord client using equivalent workloads and identical hardware.

If a target is unrealistic, measure the actual behavior and explain the technical reasons.

---

## 10. Application Architecture

Design a modular, maintainable Rust workspace.

Suggested structure:

```text
discord-native/
├── Cargo.toml
├── crates/
│   ├── app/
│   ├── core/
│   ├── auth/
│   ├── api/
│   ├── gateway/
│   ├── messaging/
│   ├── voice/
│   ├── audio/
│   ├── video/
│   ├── screen-capture/
│   ├── ui/
│   ├── rendering/
│   ├── storage/
│   ├── cache/
│   ├── notifications/
│   ├── settings/
│   └── diagnostics/
├── assets/
├── tests/
├── benchmarks/
├── docs/
└── scripts/
```

Use clear interfaces between components.

Network, audio, video, and database operations must not block the UI thread.

Implement robust error handling, session lifecycle management, cancellation, retries with backoff, and graceful shutdown.

Do not create unnecessary abstraction layers merely to increase architectural complexity.

---

## 11. Testing and Compatibility

Create automated tests and a realistic manual verification plan.

Test:

1. Application startup and shutdown.
2. Authentication and session restoration.
3. Server and channel navigation.
4. Messaging and message history.
5. Concurrent incoming messages.
6. Notifications.
7. Voice-device detection.
8. Joining and leaving voice channels.
9. Two-way communication with official Discord users.
10. Audio quality under network disruption.
11. Screen sharing and video when supported.
12. High message volumes.
13. Memory consumption over extended sessions.
14. CPU consumption during idle and active use.
15. Crashes and recovery.
16. Windows desktop integration.

Never mark tests as passed without executing them.

Use real integration tests only with authorized accounts and supported interfaces. Do not assume successful interoperability based on mocked responses.

---

## 12. Build and Distribution

Target Windows 10 and Windows 11 first.

Produce:

- A complete Rust project.
- A reproducible release build.
- A native Windows executable.
- A Windows installer where feasible.
- Configuration files.
- Automated tests.
- Benchmark utilities.
- Development documentation.
- Installation and build instructions.

Optimize release builds using suitable compiler settings, link-time optimization, and dependency review.

Avoid requiring users to install Node.js, Electron, or a separate browser runtime.

---

## 13. Development Process

Follow this order:

**Phase 1 — Research and feasibility**

Investigate Discord's current official APIs, SDKs, authentication mechanisms, real-time communication support, and policies.

Create a feature compatibility matrix.

Identify which requirements are officially supported, technically unverified, or blocked.

**Phase 2 — Architecture**

Select the native Rust UI framework, networking stack, multimedia libraries, and state-management approach.

Establish the workspace structure and build system.

**Phase 3 — Functional foundation**

Implement the native desktop shell, navigation, configuration, secure authentication where supported, and available messaging features.

**Phase 4 — Real-time systems**

Implement and test supported gateway, messaging, audio, and communication functionality.

**Phase 5 — UI parity**

Build the complete desktop interface with native rendering and optimized state updates.

**Phase 6 — Multimedia**

Implement supported audio, video, screen capture, and hardware acceleration.

**Phase 7 — Performance**

Profile the application, eliminate bottlenecks, benchmark workloads, and optimize memory, CPU, GPU, and startup behavior.

**Phase 8 — Final validation**

Run integration tests, verify compatibility, fix critical bugs, and produce an installable release.

---

## 14. Rules for the AI Development Agent

You must follow these rules throughout development:

1. **Write real, working code.** Do not substitute mockups or pseudocode for requested implementations.
2. **Do not use Electron or WebView wrappers.** The application must have a native desktop architecture.
3. **Prioritize existing Discord compatibility.** Do not silently replace Discord connectivity with an unrelated custom backend.
4. **Do not invent working integrations.** Verify all external capabilities.
5. **Do not bypass Discord security or access restrictions.**
6. **Do not copy proprietary Discord source code.** Use independently developed implementations and appropriately licensed dependencies.
7. **Do not sacrifice stability for small performance improvements.**
8. **Always benchmark meaningful optimizations.**
9. **Test functionality before marking it complete.**
10. **Do not hide limitations.** Explain any capability that cannot be implemented using authorized interfaces.
11. **Keep the code modular and maintainable.**
12. **Do not stop at planning.** After feasibility analysis, immediately implement the components that can genuinely work.
13. **Do not report the entire project as completed when only the UI or basic networking is functional.**
14. **Continue implementation through testing and packaging within the available execution environment.**

---

## 15. Definition of Done

The project may only be considered a complete Discord replacement if the resulting application:

- Runs natively on Windows without Electron.
- Provides a usable and responsive desktop interface.
- Authenticates an existing Discord account through an authorized mechanism.
- Successfully communicates with the real Discord service for its claimed features.
- Supports messaging and voice interoperability with official Discord clients.
- Handles crashes, network failures, and device changes gracefully.
- Demonstrates measurable resource-efficiency improvements.
- Passes the required integration tests.
- Can be built and launched by the user.

If official integration restrictions prevent full client compatibility, clearly state that the complete replacement objective is not achievable under those constraints. Deliver the strongest functional, policy-compliant implementation possible and provide an exact list of remaining blockers.

## Final Instruction

**Your task is to engineer a genuine, highly optimized native Rust Discord desktop client—not merely a visual imitation of Discord.**

Focus on real functionality, actual interoperability, efficient resource usage, secure architecture, and measurable performance.

Begin by researching feasibility and the currently supported Discord integration options. Then proceed directly into architecture, implementation, testing, optimization, and packaging.

The desired result is a native Windows application capable of replacing the everyday Discord desktop experience as completely as the available authorized integrations permit.
