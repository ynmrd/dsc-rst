# HANDOFF — инструкция для следующего чата

Документ для продолжения работы в новом чате или другим агентом. Последнее обновление: 2026-11-11 ~21:50 UTC. Ветка по умолчанию в репозитории — `hoplite/olous-67e0f2c3`; работа второй сессии лежит в ветке `hoplite/beroia-c64bcbde` и оформлена PR в `hoplite/olous-67e0f2c3` (напрямую в ветку по умолчанию не пушить).

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

**Весь код написан, все 16 крейтов собираются.** `cargo fmt --check` и `cargo clippy --workspace --all-targets -D warnings` чистые. Вторая сессия: `cargo test --workspace` — 406 тестов пройдено, 0 упало, 2 live-теста `#[ignore]` (запускалось в sandbox).

| Крейт | Что делает | Тесты (запускал сам, все зелёные) |
|---|---|---|
| dn-core | модель Discord v10, Id, permissions, CDN, Dispatch | 19 (+ форумные теги) |
| dn-api | REST + rate limit по бакетам, 429/5xx-ретраи, multipart, поиск, pins, посты в форумах | 32 (+1 `#[ignore]` live) |
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
| dn-client | backend: сессии, gateway, REST-команды, SQLite, уведомления, голос, автозапуск | 1 + **6 e2e** (мок REST + мок WS-gateway: полная сессия, отклонённый токен, отказ user-token, voice rejoin, правки вне окна, посты в форумах) |
| dn-ui | egui 0.37: виртуализированный чат, markdown, sidebar, members, modals, settings, onboarding, stress test, форма поста в форуме, sticker picker | 7 |
| discord-native (app) | main, `--demo`, `--self-check [--network]`, трей (Win), глобальный PTT (Win), single instance, иконка | 3 |

### Что реально проверено вживую

- `discord-native --self-check --network` в sandbox: **реальный Discord API отвечает, реальный gateway прошёл HELLO→IDENTIFY и отклонил фейковый токен кодом 4004**, как и ожидалось. Credential store в sandbox недоступен (errno 38, нет keyutils), для этого добавлен безопасный fallback: токены хранятся только в памяти на время сессии и показывается предупреждение.
- UI запускался под Xvfb в `--demo` (wgpu на программном llvmpipe). Скриншоты в чате: сервера, каналы, треды, голосовые участники, markdown, embed, NEW-разделитель, упоминания, member list, настройки, Diagnostics. Отправка сообщения через xdotool работает.
- В процессе живых запусков найдены и исправлены баги: NaN-паника при прокрутке вниз после отправки, квадраты вместо emoji, сырые `<@id>` в превью ответа, свой статус «offline».

### CI (GitHub Actions, `.github/workflows/ci.yml`)

- Linux: fmt, clippy `-D warnings`, тесты, self-check с сетью. Зелёный начиная с коммита 4d840b5.
- Windows (MSVC): тесты, release-сборка, self-check с сетью, NSIS-инсталлятор как артефакт `discord-native-windows-x64` (exe, инсталлятор, `SHA256SUMS.txt`).
- Итог первой сессии: на Windows тесты, release-сборка и self-check проходили (прогон 43093150909), падал только шаг Installer. Причина: в образе `windows-latest` (Windows Server 2025) больше нет NSIS. Во второй сессии шаг ставит NSIS через Chocolatey, если `makensis.exe` не найден.
- Статус прогонов для PR #1: `gh run list --branch hoplite/beroia-c64bcbde`, логи — `gh run view --job <id> --log-failed` (API логов иногда отвечает 502, тогда повторить позже).
- Каждый пуш в ветку с открытым PR запускает два прогона (push и pull_request). Это ожидаемо.

### Вторая сессия (2026-11-11, вечер): что сделано

- **Посты в форумах:** в форум/медиа-каналах есть форма *New Post* (заголовок, Markdown, теги, вложения) через `POST /channels/{id}/threads`. Учитываются `REQUIRE_TAG`, модерируемые теги и лимит в 5 тегов, ошибка Discord показывается в форме. Код: `dn-core` (`ForumTag`, `available_tags`, `applied_tags`), `dn-api::create_forum_post`, `Command::CreateForumPost` → `UiEvent::ForumPostResult`, `ui/src/chat.rs` (`forum_view`, `forum_post_form`).
- **Sticker picker:** кнопка 🎫 в композере отправляет стикеры текущего сервера (Discord принимает в `sticker_ids` только стикеры того же сервера).
- **Demo:** добавлены канал `help-forum` с тегами и два стикера; demo-backend обрабатывает посты и стикеры.
- **Тесты:** 3 REST-теста (JSON, multipart, валидация), разбор тегов, e2e-сценарий форума (успех, ошибка «нужен тег», нечитаемый файл). Всего 406 тестов, все проходят.
- `scripts/package-windows.sh` теперь исполняемый в git (раньше команда из README падала с Permission denied).
- **Кросс-сборка Windows проверена:** `PROFILE=release-fast scripts/package-windows.sh` собрал `dist/discord-native-0.1.0-windows-x64.exe` (41 MB) и `DiscordNative-Setup-0.1.0.exe` (11.7 MB) примерно за 10 минут. Под Wine 9.0: `--version`, `--self-check --network` (SQLite, credential store, Discord API/gateway) и `--demo` с UI работают. Wine не Windows, полноценной проверкой это не считается. Под Wine часть символов (↳, ⌄) рисуется квадратами, потому что там нет шрифтов Segoe UI; в настоящей Windows они есть.
- **Замеры в sandbox** (без GPU, llvmpipe) записаны в `docs/BENCHMARKS.md`: первый кадр 156–252 мс, idle CPU ≈ 0 %, RSS 300–420 MB (в основном программный растеризатор). Стресс-тест на 100k сообщений: 47–59 FPS, собственная работа UI p95 ≈ 5.5 мс. На реальном железе ещё не мерили.

## 3. Что осталось сделать (по приоритету)

1. **Дождаться зелёного Windows CI на PR #1** и скачать артефакт (exe + инсталлятор + SHA256SUMS). Если шаг Installer снова упадёт, посмотреть лог и путь к `makensis.exe`.
2. **Проверка с реальным ботом** (нужен токен; в sandbox его нет). Пройти разделы 1–6, 3a и 15–16 `docs/TEST_PLAN.md` на Windows, записать результаты в журнал.
3. **Голос с официальным клиентом** (пункт 9 плана). Пока не проверено, голос нельзя называть совместимым. Возможные проблемы: DAVE, согласование режима шифрования, op 4 при mute/deaf.
4. **Производительность на реальном железе:** release-сборка из CI, сравнение с официальным клиентом, заполнить таблицу в `docs/BENCHMARKS.md`.
5. **Оставшиеся ограничения:**
   - Linux: keyring `linux-native` (keyutils) не переживает перезагрузку; можно добавить secret-service.
   - Нет эхоподавления (AEC).
   - Анимированные emoji и GIF показываются первым кадром.
   - Нет управления сервером; Lottie-стикеры не рисуются.
   - Toast-клики работают, пока процесс запущен (нет COM-активатора).
   - Голос: задержка без сети — отправка около 50–80 мс, приём около 100–180 мс; пинг голосового сервера недоступен (songbird его не отдаёт); ресемплер кубический, не sinc. Авто-rejoin после сбоя есть (до 3 попыток), но с реальным Discord не проверен.
   - Для songbird включены фичи `gateway` (нужен dashmap) и `symphonia/pcm` (иначе микрофонный трек не декодируется).

**Оценка:** функциональность, реализуемая в рамках ограничений Discord, написана примерно на 90%. Проверено: сборка и тесты на Linux и Windows CI, кросс-сборка с инсталлятором, demo UI, сетевой self-check. Не проверено: реальный бот на Windows, двусторонний голос, замеры на реальном железе. Для этого нужны Windows-машина и токен бота.

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
- Пушить в рабочую ветку, не в ветку по умолчанию: `git push origin HEAD:refs/heads/hoplite/beroia-c64bcbde` (PR #1 в `hoplite/olous-67e0f2c3`).
- Политика sandbox блокирует и динамически сгенерированные команды (например, `$((16#...))`). `pkill -f` с именем exe в командной строке убивает собственный шелл.
- Release-логи пишутся только в файл: в portable-режиме это `$DN_HOME/logs`. Кросс-сборка: `rustup target add x86_64-pc-windows-gnu`, пакеты `mingw-w64 nsis`; проверка под Wine: `apt-get install wine64`.

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
