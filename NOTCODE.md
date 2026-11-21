# NotCode Desktop — инструкции агенту

## Стек
Tauri 2 (Rust бэкенд в `src-tauri/src/`) + React/TypeScript фронт (`src/`), Monaco-редактор, xterm.js. Стиль — тёмная тема, акцент `#C4F16B`, без «сайта»: свой фреймлесс-тайтлбар, контекстные меню, user-select:none.

## Сборка и проверка
- Фронт: `npx tsc --noEmit` (типы), `npm run build` — vite (в песочнице ставить TMP/TEMP внутрь проекта: esbuild не пишет в Temp).
- Rust: `CARGO_TARGET_DIR=C:/nctarget cargo check` (короткий ASCII-путь — ring падает на кириллических путях), тесты `cargo test --lib`.
- Полный бандл: `npm run tauri build` → NSIS в `src-tauri/target/release/bundle/nsis/`.

## Правила проекта
- Новые Tauri-команды: функция в `src-tauri/src/*.rs` с `#[tauri::command]` + регистрация в `src-tauri/src/lib.rs` (`generate_handler!`) + обёртка в `src/lib/native.ts`. Не забывать: gate проекта (`project_gate`), лимиты размеров, подтверждения на запись/команды.
- Секреты — только в Windows Credential Manager (keyring), не в SQLite и не в файлах.
- Цветовые токены добавлять в `src-tauri/design-system/tokens.json` и регенерировать `npm run tokens`.
- Русские строки — по умолчанию в UI; коммиты — короткий заголовок + подробности.
- Версия синхронно в трёх местах: `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`.
