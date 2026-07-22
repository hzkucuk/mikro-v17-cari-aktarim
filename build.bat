@echo off
setlocal
cd /d "%~dp0src-tauri"

where cargo >nul 2>nul || (
  echo Rust/Cargo bulunamadi. https://rustup.rs adresinden kurun.
  exit /b 1
)

cargo tauri --version >nul 2>nul || (
  echo Tauri CLI bulunamadi. Bir kez su komutu calistirin:
  echo   cargo install tauri-cli --version "^2" --locked
  exit /b 1
)

cargo tauri build --release
endlocal
