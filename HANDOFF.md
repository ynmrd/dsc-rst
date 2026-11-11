# HANDOFF — инструкция для следующего чата

Документ для продолжения работы в новом чате или другим агентом. Последнее обновление: 2026-11-11, ветка `hoplite/olous-67e0f2c3`.

**Задача:** нативный клиент Discord на Rust для Windows 10/11 по ТЗ из [`docs/ORIGINAL_PROMPT.md`](docs/ORIGINAL_PROMPT.md). Используются только официальные интерфейсы Discord, без user-token и self-bot. Итоги исследования и таблица возможностей лежат в [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md); прочитать её нужно в первую очередь.

---

## 1. Главное решение (не менять без причины)

Полноценная замена официального клиента недостижима легальными средствами: ни один OAuth2-scope не даёт писать или читать сообщения от имени пользователя, а self-bot запрещён ToS. Поэтому клиент:

- **основной режим** — подключение **бота пользователя**. Пользователь создаёт своё приложение в Developer Portal и вставляет bot token, который хранится в Windows Credential Manager. Клиент работает через Bot API v10 и gateway v10: серверы, куда добавлен бот, и DM, которые пишут боту;
- **дополнительно** — «Sign in with Discord» (OAuth2 + PKCE, loopback `http://127.0.0.1:53134/callback`). Это даёт реальный профиль и список серверов пользователя и упрощает приглашение бота;
- **голос** работает через songbird 0.6.1 (поддерживает обязательный с 2026-03-01 DAVE E2EE). Видео и демонстрация экрана заблокированы: API для этого нет;
- при попытке вставить токен обычного аккаунта клиент выдаёт ошибку `NotABot`. Пароли и user token не запрашиваются никогда.

Это нужно честно объяснять в UI (онбординг и вкладка «About & Compatibility» в настройках) и в README.

## 2. Окружение (sandbox Ubuntu 24.04, 2 vCPU / 8 GB)

```bash
# Rust (cargo в sandbox изначально нет)
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy -c rustfmt
source ~/.cargo/env
rustup target add x86_64-pc-windows-gnu
# Системные пакеты (sudo без пароля)
sudo apt-get install -y cmake mingw-w64 nsis libasound2-dev libxkbcommon-dev libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev mesa-vulkan-drivers \
  libvulkan1 libdbus-1-dev libssl-dev xvfb clang
```

Особенности окружения:

- Сборка медленная: полная сборка UI-зависимостей (eframe и wgpu) занимает около 5 минут. **Тесты запускать по крейтам** (`cargo test -p dn-xxx`), а не всем workspace.
- Политика sandbox блокирует inline-скрипты интерпретаторов (`python3 - <<EOF`) и команды с `xargs`. Править файлы через Edit/Write.
- Пушить только в свою ветку с явным refspec: `git push origin HEAD:refs/heads/hoplite/olous-67e0f2c3`.

## 3. Структура workspace

```
Cargo.toml            # workspace, release: fat LTO, codegen-units=1, strip
crates/
  core/          dn-core           модель Discord v10, Id, permissions, CDN, Dispatch
  api/           dn-api            REST-клиент + rate limiting по бакетам
  gateway/       dn-gateway        gateway v10: heartbeat, identify/resume, zlib-stream, backoff
  auth/          dn-auth           OAuth2 PKCE, bot-аккаунты, keyring, AccountManager
  storage/       dn-storage        SQLite (WAL), FTS5-поиск, read states, prune, поток-хэндл
  cache/         dn-cache          Store (состояние клиента) + bus (Command/UiEvent)
  messaging/     dn-messaging      Discord-markdown → AST, emoji, автодополнение, группировка
  settings/      dn-settings       Settings + Paths (%APPDATA%\DiscordNative, DN_HOME = portable)
  diagnostics/   dn-diagnostics    логи (rolling), crash-репорты, метрики RSS/CPU, FrameStats
  audio/         dn-audio          CPAL, DSP (VAD/PTT/AGC/RNNoise), ресемплинг
  voice/         dn-voice          songbird driver, микширование, speaking-индикаторы
  media/         dn-media          асинхронная загрузка и декодирование картинок, LRU-кэши
  notifications/ dn-notifications  правила уведомлений + Windows toasts
  client/        dn-client         backend-оркестратор (связывает всё с UI)
  ui/            dn-ui             egui 0.37 UI
  app/           discord-native    бинарник: main, окно, трей, single-instance
docs/  COMPATIBILITY.md, ORIGINAL_PROMPT.md
```

## 4. Статус на момент записи

| Крейт | Статус | Проверка |
|---|---|---|
| dn-core | ✅ готов | `cargo test -p dn-core` — 18 passed |
| dn-cache | ✅ готов (Store, views, bus) | 16 passed |
| dn-api | ✅ готов | 29 passed; 1 live-тест `#[ignore]` (GET /gateway) я сам не запускал |
| dn-gateway | ✅ готов | 16 passed; live-тест `#[ignore]` (ожидает close 4004 на фейковом токене) я сам не запускал |
| dn-auth | ✅ готов | 60 passed (mock OAuth-сервер, MemoryStore) |
| dn-storage | ✅ готов | 24 passed; есть criterion-бенчмарк |
| dn-settings | ✅ готов | 16 passed |
| dn-diagnostics | ✅ готов | 17 passed |
| dn-messaging | ✅ готов | 62 passed (вкл. фаззинг и 200 KB вход) |
| dn-audio, dn-voice | ⏳ в работе (сабагент) | — |
| dn-media, dn-notifications | ✅ готовы | по отчёту сабагента: 40 + 33 теста, clippy и Windows cross-check чистые (я их не перезапускал) |
| dn-ui | 🟢 код написан, `cargo check` и `clippy` чистые | тесты dn-ui в процессе запуска |
| dn-client | 🟡 код написан (`lib.rs`, `commands.rs`, `voice_glue.rs`, `desktop.rs`) по плану из раздела 6, **ещё не компилировался**: ждёт dn-audio/dn-voice | e2e-тест ещё не написан |
| app | 🟡 код написан (`main.rs`, `demo.rs` (`--demo`), `instance.rs`, `tray.rs`, `ptt.rs`, `selfcheck.rs`, `build.rs`), иконки в `assets/` (генерируются `scripts/make_icon.py`), **ещё не компилировался** | — |
| CI, NSIS-инсталлятор, README, бенчмарки vs официальный клиент | ❌ | раздел 8 |

Все тесты выше запускал я в этой сессии. В Windows ничего ещё не запускалось. Реальное подключение к Discord не проверялось: в sandbox нет токена бота.

### Контракты API, которые должны сохраняться (UI уже пишет код под них)

- **dn-audio:** `AudioEngine` (Clone; `new(AudioConfig)`, `set_config`, `start/stop_capture`, `start/stop_playback`, `capture_frames() -> CaptureReader`, `playback_writer() -> PlaybackWriter`, `set_ptt/set_muted/set_deafened`, `input_level_db()`, `voice_active()`, `events()`, `set_monitor`, `shutdown`), а также `input_devices()/output_devices() -> Vec<DeviceInfo{name,is_default}>`.
- **dn-voice:** `VoiceManager::new(AudioEngine, UnboundedSender<VoiceEvent>)`, `connect(VoiceConnectInfo{guild_id,channel_id,user_id,session_id,endpoint,token})`, `disconnect()`, `set_self_mute/deaf`, `set_user_volume`, `set_bitrate`; `VoiceEvent::{State(VoiceState), Speaking{user_id,speaking}, SelfSpeaking(bool), Ping(Option<u32>), UserLeft{user_id}, Error(String)}`.
- **dn-media:** `MediaLoader::new(MediaConfig, runtime::Handle, Arc<dyn Fn()+Send+Sync>)`, `request(url,max_dim)`, `forget`, `drain() -> Vec<MediaEvent{Ready(DecodedImage{url,width,height,rgba}) | Failed{url,error}}>`, `cancel_all`; `BoundedCache<K,V>::{new,get,peek,insert->evicted,remove,len,bytes,set_budget,clear}`.
- **dn-notifications:** `should_notify(&Message, &NotifyContext)`, `build_notification(msg, author, location, show_preview)`, `Notifier::{new(name, icon), show(&Notification), on_activated() -> Receiver<Id>}`.

Если сабагенты не закончили (крейт пустой или не проходит тесты), реализовать по этим контрактам. Полные спецификации для сабагентов были такими же, как контракты выше, плюс тесты на DSP и микшер, wiremock-тесты загрузчика и матрицу `should_notify`.

## 5. Ключевые решения в коде

- **Store** (`dn-cache`) — `Arc<parking_lot::Mutex<Store>>`. Backend применяет `Dispatch` и результаты REST, UI читает его каждый кадр. Окна сообщений ограничены: открытый канал до 3000 сообщений, неактивный до 100, в памяти не больше 24 каналов (LRU). При прокрутке назад обрезается новый конец и ставится флаг `has_newer_gap` (UI показывает «Jump to Present»).
- **Bus** (`dn_cache::bus`): UI отправляет `Command` в unbounded tokio mpsc, backend отвечает `UiEvent` через ограниченный crossbeam-канал (512) и вызывает `Repaint`.
- **Read state** у ботов на сервере нет, поэтому он хранится локально в SQLite. При первом появлении канала существующая история считается прочитанной.
- **UI (egui 0.37):** `App::ui(&mut Ui)` + `App::logic()` (вызывается и при скрытом окне: трей, close-to-tray). Виртуализация: `crates/ui/src/virtual_list.rs` (кэш измеренных высот с сигнатурой сообщения, префиксные суммы, бинпоиск видимого диапазона, якорение при подгрузке старых сообщений). Markdown рендерится в `LayoutJob` с хит-тестом по символам (ссылки, упоминания, спойлеры), кастомные emoji рисуются поверх NBSP. Текстуры живут в LRU по байтам, загрузка на GPU не больше 8 за кадр.
- **Idle CPU:** egui перерисовывает только по событиям; таймеры (тосты, typing) ставят `request_repaint_after`.

## 6. Следующий шаг: `dn-client` (backend)

Нужна функция `dn_client::start(rt: &Handle, cfg: BackendConfig, link: BackendLink, cmd_rx: UnboundedReceiver<Command>) -> JoinHandle<()>`, где
`BackendConfig { paths: Paths, api_base: String, gateway_url: Option<String>, gateway_timings: Option<Timings>, secret_store: Arc<dyn SecretStore>, notifications: bool, audio: bool }`.
Переопределяемые URL нужны для e2e-теста с моком Discord.

Главный цикл — `tokio::select!` по: командам UI; событиям gateway (с номером сессии, чтобы отбрасывать события старого gateway); `VoiceEvent`; внутренним сообщениям от spawned-задач; тику раз в 1 с (батч записи сообщений в SQLite, сохранение read states и настроек, раз в 10 минут `prune`); тику 50 мс, только в голосе или при тесте микрофона (уровень микрофона в Store).

Обработка команд:

- **Аккаунты.** `AccountManager` лежит в `Arc<tokio::sync::Mutex<…>>`.
  - `AddBotAccount`: `add_bot_account(token, api_base)`, затем `UiEvent::AccountAdded`, затем старт сессии.
  - `SwitchAccount`: `set_active`, затем перезапуск сессии.
  - `RemoveAccount`: удаляет аккаунт и его секреты.
  - `BeginOAuth`/`CancelOAuth`: `oauth::sign_in` с oneshot-отменой, затем `fetch_user`, `link_oauth`, `RestClient(Bearer).current_user_guilds()`, после чего заполняется `store.session.linked`.
  - `UnlinkOAuth`: снимает OAuth-привязку.
  - Список аккаунтов показывать в `store.session.accounts` (`AccountView`). Если аккаунтов нет, ставить `needs_onboarding = true`.
- **Старт сессии.**
  1. Остановить старый gateway и голос, выполнить `store.reset_session_data()`.
  2. Взять токен (`bot_token`), создать `RestClient::new(Auth::Bot)`.
  3. Из SQLite загрузить `private_channels()` и `read_states()`.
  4. URL взять через `rest.gateway_bot()` (fallback — `DEFAULT_GATEWAY_URL`).
  5. Intents: `Intents::default_client()` минус выключенные в настройках привилегированные.
  6. Выполнить `dn_gateway::spawn`.
- **События gateway.**
  - `State` → `session.conn`; `Latency` → `latency_ms`.
  - `Dispatch` → `store.apply(&d)`, затем побочные эффекты:
    - сохранить `MESSAGE_CREATE/UPDATE/DELETE` в SQLite;
    - DM-каналы сохранять через `upsert_private_channel`;
    - уведомления: `should_notify` → `Notifier::show`, а при упоминании и неактивном окне ещё и `UiEvent::RequestAttention`;
    - `take_dirty_read_states()` → `set_read_state`.
  - `Fatal(DisallowedIntents)` → переподключиться без привилегированных intents и выставить `intents_degraded = true`.
  - `Fatal(AuthenticationFailed)` → `mark_credentials_invalid`, `conn = Fatal("Bot token rejected…")`, `needs_onboarding`.
- **Сообщения.**
  - `OpenChannel`: если окно не загружено, сначала мгновенно отдать кэш из SQLite (`load_messages`), затем REST `messages(MessagePage::latest(50))`. После ответа удалить из окна сообщения в диапазоне страницы, которых в ней нет (они были удалены, пока клиент был офлайн), и вызвать `messages_loaded(.., None, len<50)`. Не перезапрашивать чаще раза в 30 с.
  - `LoadOlder`: `before`, 50 сообщений; при ошибке `set_loading_older(false)` и тост.
  - `JumpToPresent`: последняя страница.
  - `JumpToMessage`: `around` → `messages_loaded_around`.
  - `SendMessage`: nonce = случайный u64 → `store.add_pending`. Файлы читать через `tokio::fs`. Затем `create_message`, ответ применить как `Dispatch::MessageCreate` (дедуп по id и nonce), при ошибке `fail_pending`. Для `RetrySend` хранить параметры в map по nonce.
  - `EditMessage`, `DeleteMessage`, `React`, `Pin`, `LoadPins` (→ `UiEvent::Pins`), `Typing` (ошибки игнорировать), `MarkRead`.
  - `Search`: `search_guild_messages`. На `Indexing` повторить до 3 раз через `retry_after`. При ошибке или отсутствии MESSAGE_CONTENT откатиться на локальный `storage.search` с `note`.
  - `OpenDm`: `create_dm` → `UiEvent::OpenChannel`.
  - `CreateThread`, `DownloadTo` (`download` + `fs::write` + тост), `FetchProfile` (→ `UiEvent::Profile`).
  - `RequestMembers`: gateway op 8 (`query ""`, `limit 0`), только если есть GUILD_MEMBERS.
  - `SetPresence`: op 3 + запись своей presence в Store.
- **Голос.**
  - `JoinVoice` → `conn = Connecting`, отправить op 4 `UpdateVoiceState{channel_id: Some}`.
  - Собрать `session_id` из своего `VOICE_STATE_UPDATE` и `token/endpoint` из `VOICE_SERVER_UPDATE` (`endpoint` может быть `null`, тогда ждать следующий), затем `VoiceManager::connect`. Повторный `VOICE_SERVER_UPDATE` означает переподключение.
  - `LeaveVoice` → op 4 с `channel_id: None` + `disconnect`.
  - `SetMute/SetDeaf` → voice + op 4, чтобы иконка обновилась у других.
  - `PushToTalk` → `audio.set_ptt`.
  - `MicTest` → `set_monitor` + start/stop capture и playback.
  - `RefreshAudioDevices` → `store.session.audio_inputs/outputs`.
- **Прочее.**
  - `SettingsChanged`: отложенное сохранение, применить `AudioConfig`, автозапуск (Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, значение `DiscordNative` = `"<exe>" --minimized`; Linux: `~/.config/autostart/discord-native.desktop`).
  - `FocusChanged`.
  - `Reconnect`.
  - `Shutdown`: сбросить буферы SQLite, сохранить настройки, `gateway.shutdown()`, отключить голос, `audio.shutdown`, выйти из цикла.
- **Клики по тостам:** `notifier.on_activated()` → `UiEvent::OpenChannel`. UI при этом должен показать окно и дать ему фокус.

**Тест e2e** (`crates/client/tests/`): wiremock-REST + локальный WS-gateway. Проверить:

- `AddBotAccount` (MemoryStore);
- READY + GUILD_CREATE → Store;
- `OpenChannel` → сообщения;
- `SendMessage` → pending и echo по nonce;
- `MESSAGE_CREATE` → SQLite;
- close 4004 → `credentials_invalid`.

## 7. `app` (бинарник `discord-native`)

1. `Paths::detect().ensure()` → `Settings::load` → `init_logging` + `install_panic_hook` + `SessionMarker::begin` (при некорректном завершении прошлой сессии показать тост со ссылкой на crash-репорт).
2. Tokio runtime с 2 worker-потоками → `Store::shared(limits из настроек)` → `bus::link` → `MediaLoader` → `dn_client::start`.
3. `eframe::run_native`:
   - рендерер `Renderer::Wgpu`, если включено аппаратное ускорение, иначе `Glow`;
   - размер окна из настроек, минимальный 940×500, иконка;
   - `--minimized` → `Visible(false)` (только если есть трей).
4. **Трей (только Windows,** `tray-icon` 0.26): создаётся внутри creator-замыкания eframe. Меню: «Open», «Quit». В `set_event_handler` вызывать `ctx.request_repaint()` и передавать событие в канал. В `App::logic` при «Open» отправить `Visible(true)` + `Focus`, при «Quit» — `request_quit()` + `Close`.
5. **Single instance:** TCP `127.0.0.1:(40000 + hash(data_dir) % 20000)`. Второй экземпляр шлёт `show` и завершается.
6. **Глобальный PTT на Windows:** поток опрашивает `GetAsyncKeyState` каждые 10 мс, только в голосе и в режиме PTT. Внутриоконный PTT в UI при этом надо отключить (добавить флаг в `UiOptions`).
7. CLI: `--version`, `--self-check` (офлайн-проверки подсистем; пригодится в CI на Windows), `--demo` (офлайн-данные для скриншотов и стресс-теста, в UI помечено как «Offline demo — not connected to Discord»).
8. `build.rs`: на Windows `winresource` (иконка и версия в exe); в `main.rs` `#![windows_subsystem = "windows"]` для release.

## 8. После этого

1. Собрать `dn-ui`, исправить ошибки компиляции. Добавить юнит-тесты для `virtual_list` (уже есть), `modals::score` (есть), `widgets::human_size` (есть).
2. Запустить под Xvfb (`xvfb-run`, glow/llvmpipe или wgpu/lavapipe) с `--demo`, снять скриншоты (`import -window root` из imagemagick или xwd) и посмотреть их глазами. Прогнать стресс-тест из Settings → Diagnostics (10k/50k/100k сообщений) и записать FPS. Замерить idle RSS/CPU.
3. Кросс-сборка для Windows: `cargo build --release --target x86_64-pc-windows-gnu -p discord-native`. NSIS-скрипт `installer/discord-native.nsi` (`makensis` уже установлен) → `dist/DiscordNative-Setup-<ver>.exe`.
4. `.github/workflows/ci.yml`: тесты на ubuntu и windows-latest (MSVC), clippy, release-сборка и инсталлятор как артефакты.
5. Документация:
   - `README.md` — установка, создание бота, включение intents, ограничения;
   - `docs/ARCHITECTURE.md`;
   - `docs/TEST_PLAN.md` — ручной план по 16 пунктам ТЗ, особенно двусторонний голос с официальным клиентом;
   - `docs/BENCHMARKS.md` — только реально измеренное, и с пометкой, что сравнение с официальным клиентом на одном железе ещё не сделано.
6. Финальный отчёт пользователю: что работает, что не проверено (голос с официальными клиентами, работа на Windows) и что заблокировано Discord.

## 9. Правила (из ТЗ, обязательны)

- Не выдавать непроверенное за работающее. Голос «реализован, не проверен» до реального двустороннего теста с официальным клиентом.
- Не использовать Electron, Tauri или WebView. Не копировать ассеты и код Discord.
- Тесты не помечать пройденными без запуска. Не логировать токены (`Debug` у секретов уже редактирует их).
