@echo off
setlocal
pushd "%~dp0"

where pnpm >nul 2>&1 || (echo [ERROR] pnpm is required - see packageManager in package.json.& exit /b 1)
where cargo >nul 2>&1 || (echo [ERROR] Rust/Cargo is required - see rust-version in Cargo.toml.& exit /b 1)

echo [Ruvyxa] Installing workspace dependencies...
call pnpm install --frozen-lockfile || exit /b 1
echo [Ruvyxa] Building workspace packages...
call pnpm -r build || exit /b 1
echo [Ruvyxa] Compiling the Ruvyxa CLI...
call cargo build --locked -p ruvyxa_cli || exit /b 1

echo.
echo Setup complete. Start developing with:
echo   cd examples\demo
echo   pnpm dev
popd
exit /b 0
