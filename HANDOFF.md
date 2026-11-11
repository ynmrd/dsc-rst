# HANDOFF — инструкция для следующего чата

Документ для продолжения работы в новом чате или другим агентом. Последнее обновление: 2026-11-11 ~19:55 UTC, ветка `hoplite/olous-67e0f2c3`.

**Задача:** нативный клиент Discord на Rust для Windows 10/11 по ТЗ из [`docs/ORIGINAL_PROMPT.md`](docs/ORIGINAL_PROMPT.md). Используются только официальные интерфейсы Discord, без user-token и self-bot.

Сначала прочитать:

- [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md) — исследование и таблица «что возможно, а что заблокировано»;
- [`README.md`](README.md) — как пользоваться;
- [`docs/TEST_PLAN.md`](docs/TEST_PLAN.md) — ручной план тестирования.

---

## 1. Главное решение (не менять без причины)

Полноценная замена официального клиента невозможна легальными средствами: ни один OAuth2-scope не даёт писать или читать сообщения от имени пользователя, а self-bot запрещён ToS. Поэтому клиент:

- **основной режим** — **бот пользователя**. Пользователь создаёт своё приложение в Developer Portal и вставляет bot token, который хранится в Windows Credential Manager. Клиент работает через Bot API v10 и gateway v10: серверы, куда добавлен бот, и DM, которые пишут боту;
- **дополнительно** — «Sign in with Discord» (OAuth2 + PKCE, `http://127.0.0.1:53134/callback`). Даёт настоящий профиль и список серверов;
- **голос** — songbird 0.6.1 с DAVE E2EE (обязательно с 2026-03-01). **Видео и демонстрация экрана заблокированы** (API нет);
- токен обычного аккаунта отклоняется (`NotABot`). Пароли и user token не запрашиваются никогда.

## 2. Текущее состояние

**Весь код написан, все 16 крейтов собираются.** `cargo fmt --check` и `cargo clippy --workspace --all-targets -D warnings` чистые.

| Крейт | Что делает | Тесты (запускал сам, все зелёные) |
|---|---|---|
| dn-core | модель Discord v10, Id, permissions, CDN, Dispatch | 18 |
| dn-api | REST + rate limit по бакетам, 429/5xx-ретраи, multipart, поиск, pins | 29 (+1 `#[ignore]` live) |
| dn-gateway | gateway v10: heartbeat/zombie, identify/resume, zlib-stream, backoff, fatal-коды | 16 (+1 `#[ignore]` live) |
| dn-auth | OAuth2 PKCE loopback, AccountManager, keyring, refresh/revoke | 60 |
| dn-storage | SQLite WAL + FTS5, read states, prune, восстановление при повреждении | 24 + бенчмарк |
| dn-cache | Store (состояние) + bus (Command/UiEvent), ограниченные окна сообщений | 16 + бенчмарк |
| dn-messaging | Discord-markdown → AST, emoji, автодополнение, группировка | 62 + бенчмарк |
| dn-settings / dn-diagnostics | настройки и пути / логи, crash-репорты, метрики | 16 / 17 |
| dn-media | загрузка и декодирование картинок, LRU по байтам, диск-кэш | 40 (по отчёту сабагента) |
| dn-notifications | правила, rate limit, Windows toasts с кликом | 33 (по отчёту сабагента) |
| dn-audio | CPAL, VAD/PTT, RNNoise, AGC, ресемплинг, восстановление устройства | 33 (по отчёту сабагента) |
| dn-voice | songbird driver, mic source, микшер, speaking | 21 (по отчёту сабагента) |
| dn-client | backend: сессии, gateway, REST-команды, SQLite, уведомления, голос, автозапуск | 1 + **3 e2e** (мок REST + мок WS-gateway: полная сессия, отклонённый токен, отказ user-token) |
| dn-ui | egui 0.37: виртуализированный чат, markdown, sidebar, members, modals, settings, onboarding, stress test | 7 |
| discord-native (app) | main, `--demo`, `--self-check [--network]`, трей (Win), глобальный PTT (Win), single instance, иконка | 3 |

### Что реально проверено вживую

- `discord-native --self-check --network` в sandbox: **реальный Discord API отвечает, реальный gateway прошёл HELLO→IDENTIFY и отклонил фейковый токен кодом 4004**, как и ожидалось. Credential store в sandbox недоступен (errno 38, нет keyutils), для этого добавлен безопасный fallback: токены хранятся только в памяти на время сессии и показывается предупреждение.
- UI запускался под Xvfb в `--demo` (wgpu на программном llvmpipe). Скриншоты в чате: сервера, каналы, треды, голосовые участники, markdown, embed, NEW-разделитель, упоминания, member list, настройки, Diagnostics. Отправка сообщения через xdotool работает.
- В процессе живых запусков найдены и исправлены баги: NaN-паника при прокрутке вниз после отправки, квадраты вместо emoji, сырые `<@id>` в превью ответа, свой статус «offline».

### CI (GitHub Actions, `.github/workflows/ci.yml`)

- Linux: fmt, clippy `-D warnings`, тесты, self-check с сетью.
- Windows (MSVC): тесты, release-сборка, self-check с сетью, NSIS-инсталлятор как артефакт.
- Первый прогон (коммит d3393d1): Linux упал только на шаге Format (исправлено следующим коммитом с `rustfmt.toml` и `cargo fmt`); Windows на момент записи ещё шёл. **Проверить:** `gh run list --branch hoplite/olous-67e0f2c3`, затем `gh run view <id> --log-failed`.

## 3. Что осталось сделать (по приоритету)

1. **Довести CI до зелёного на Windows.** Это первая настоящая сборка и прогон тестов на Windows: возможны ошибки линковки или тестов, специфичные для MSVC. Скачать артефакт (exe + инсталлятор).
2. **Проверка с реальным ботом** (нужен токен; в sandbox его нет). Добавить бота, открыть каналы, отправить, редактировать и удалить сообщения, реакции, пины, поиск, DM, треды, уведомления. Пройти разделы 1–6 и 15–16 из `docs/TEST_PLAN.md` на Windows.
3. **Голос с официальным клиентом** (пункт 9 плана). Пока не проверено, голос нельзя называть совместимым. Возможные проблемы: DAVE, согласование режима шифрования, op 4 при mute/deaf.
4. **Измерения производительности:** release-сборка на реальном железе — idle RAM/CPU, холодный старт, стресс-тест (Settings → Diagnostics), сравнение с официальным клиентом. Записать в `docs/BENCHMARKS.md` (файла ещё нет). Пока есть только debug-сборка под программным рендером (416 MB RSS), это **не показательно**.
5. **Локальная кросс-сборка Windows + NSIS:** `scripts/package-windows.sh` написан, но ещё не запускался (mingw, nsis и target уже установлены в sandbox).
6. **Мелочи и ограничения, которые можно улучшить:**
   - Linux: keyring `linux-native` (keyutils) не переживает перезагрузку; добавить secret-service.
   - Нет эхоподавления (AEC).
   - Анимированные emoji и GIF показываются первым кадром.
   - Нет создания постов в форумах, отправки стикеров через picker и управления сервером.
   - Toast-клики работают, пока процесс запущен (нет COM-активатора).
   - `MESSAGE_UPDATE` для сообщений вне окна в памяти не обновляет SQLite-кэш.
   - Голос (по отчёту сабагента):
     - задержка без сети: отправка около 50–80 мс, приём около 100–180 мс (songbird playout ~100 мс + наш prebuffer 40 мс);
     - пинг голосового сервера всегда пустой: songbird его не отдаёт;
     - после `Failed` (например, close 4014) backend выходит из канала, и пользователь заходит заново. Можно сделать авто-reconnect по следующему `VOICE_SERVER_UPDATE`;
     - ресемплер кубический, не sinc;
     - для songbird включены фичи `gateway` (нужен dashmap) и `symphonia/pcm` (иначе микрофонный трек не декодируется).
7. Обновить этот файл и README по итогам пунктов 1–4, а пользователю дать финальный отчёт (что работает, что не проверено, что заблокировано Discord).

**Оценка:** код по реализуемым под ограничениями Discord функциям готов примерно на 85–90%. Проверка на реальной Windows, с реальным ботом, двусторонний голос и замеры производительности ещё впереди (сделано примерно 30%). Эти пункты требуют Windows-машины и токена бота, в sandbox их не выполнить.

## 4. Окружение sandbox (Ubuntu 24.04, 2 vCPU / 8 GB)

```bash
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy -c rustfmt
source ~/.cargo/env && rustup target add x86_64-pc-windows-gnu
sudo apt-get install -y cmake mingw-w64 nsis libasound2-dev libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev mesa-vulkan-drivers libvulkan1 libdbus-1-dev \
  libssl-dev xvfb clang imagemagick x11-apps xdotool
```

Как работать:

- Сборка медленная. Тесты запускать по крейтам (`cargo test -p dn-xxx`). Полный `cargo test --workspace` идёт дольше 2 минут, поэтому запускать с большим timeout.
- **UI-проверка:**
  ```bash
  Xvfb :99 -screen 0 1440x900x24 &
  DISPLAY=:99 DN_HOME=/tmp/dn-demo target/debug/discord-native --demo &
  # скриншот: import -window root shot.png; клики: xdotool
  ```
  Перед `xdotool type` выполнить `xdotool windowfocus`.
- `pkill -f "discord-native --demo"` убивает и собственный шелл. Использовать `pkill -x discord-native`.
- Политика sandbox блокирует inline-скрипты интерпретаторов (`python3 - <<EOF`) и `xargs`. Править файлы через Edit/Write.
- Пушить так: `git push origin HEAD:refs/heads/hoplite/olous-67e0f2c3`.

## 5. Ключевые решения в коде

- **Store** (`dn-cache`) — `Arc<parking_lot::Mutex<Store>>`. Backend применяет Dispatch и REST, UI читает каждый кадр. Окна сообщений ограничены: открытый канал до 3000, неактивный до 100, в памяти не больше 24 каналов. При прокрутке назад новый конец обрезается и ставится флаг `has_newer_gap` (кнопка «Jump to Present»). Последняя страница без пересечения с окном заменяет его, чтобы не было «дыр».
- **Bus:** `Command` идут через tokio mpsc, `UiEvent` — через ограниченный crossbeam-канал, плюс `Repaint`. Тяжёлые варианты `UiEvent` упакованы в `Box`.
- **Backend** (`crates/client/src/lib.rs`): один `tokio::select!`-цикл. REST-вызовы выполняются в spawned-задачах. События gateway помечены номером сессии, чтобы отбрасывать события старого gateway. Голосовой handshake: op 4 → `VOICE_STATE_UPDATE` (session_id) + `VOICE_SERVER_UPDATE` (token/endpoint) → `VoiceManager::connect`. Если голос не подключился за 15 с, срабатывает таймаут.
- **UI (egui 0.37):** `App::ui` + `App::logic` (вызывается и при скрытом окне). Виртуализация в `ui/src/virtual_list.rs`, Markdown — `LayoutJob` с хит-тестом по символам. Текстуры в LRU по байтам, загрузка не больше 8 за кадр. В простое кадры не рисуются.
- **Безопасность:** у токенов в `Debug` значения скрыты. Токены не пишутся на диск (e2e-тест это проверяет). Внешние ссылки открываются через предупреждение.

## 6. Правила (из ТЗ, обязательны)

- Не выдавать непроверенное за работающее. Голос «реализован, не проверен» до реального двустороннего теста.
- Не использовать Electron, Tauri или WebView. Не копировать ассеты и код Discord (иконка своя, `scripts/make_icon.py`).
- Тесты не помечать пройденными без запуска. Ручные прогоны записывать в журнал в `docs/TEST_PLAN.md`.
