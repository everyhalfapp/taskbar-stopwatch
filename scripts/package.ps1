# 배포용 zip 만들기
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1
# 결과: dist\TaskbarStopwatch-v<버전>-windows-x64.zip (+ .sha256)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Taskbar Stopwatch v$version"

# 빌드한 PC 의 사용자 폴더·프로젝트 경로가 exe 안에 남지 않도록 경로를 바꿔서 컴파일
$sep = [char]0x1f
# (CARGO_ENCODED_RUSTFLAGS 는 .cargo/config.toml 의 rustflags 를 덮어쓰므로 그 플래그도 함께 넣음)
$flags = @("-Ctarget-feature=+crt-static", "-Ccontrol-flow-guard",
           "--remap-path-prefix=$env:USERPROFILE=~", "--remap-path-prefix=$root=.")
if ($env:CARGO_HOME) { $flags += "--remap-path-prefix=$env:CARGO_HOME=~/.cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join $sep

cargo test --release
if ($LASTEXITCODE -ne 0) { throw 'cargo test 실패' }
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build 실패' }

$name = "TaskbarStopwatch-v$version-windows-x64"
$stage = Join-Path $root "dist\$name"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\TaskbarStopwatch.exe $stage -Force
Copy-Item LICENSE (Join-Path $stage 'LICENSE.txt') -Force

$readme = @"
Taskbar Stopwatch v$version
===========================

작업표시줄 위에 작게 붙여 두고 실제 작업 시간을 재는 스톱워치 위젯입니다.

실행
  TaskbarStopwatch.exe 를 원하는 폴더에 두고 실행하세요. 설치 과정은 없습니다.
  "Windows의 PC 보호" 창이 뜨면 [추가 정보] -> [실행] 을 누르세요.

사용법
  시작/일시정지 ... 재생 버튼
  초기화 .......... 초기화 버튼을 두 번 클릭
  시간 직접 입력 .. 시간 부분 더블클릭 -> 입력 -> Enter
                    예) 1:30:00 / 1:30 / 45(분) / +10 / -5m / 1h20m
  이동 ............ 끌어서 이동 (작업표시줄 근처에서 자동 정렬)
  크기 ............ 오른쪽 끝을 끌기, Ctrl + 마우스 휠
  설정 ............ 우클릭 -> 설정...
  종료 ............ 우클릭 -> 종료 (트레이 아이콘 우클릭도 같음)

데이터 위치
  %APPDATA%\TaskbarStopwatch\

라이선스: MIT (LICENSE.txt)
"@
[IO.File]::WriteAllText((Join-Path $stage 'README.txt'), $readme, [Text.UTF8Encoding]::new($true))

$zip = Join-Path $root "dist\$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
# sha256sum -c 로 바로 검증할 수 있게 LF 줄바꿈 + BOM 없이
[IO.File]::WriteAllText("$zip.sha256", "$hash  $name.zip`n", [Text.UTF8Encoding]::new($false))

Write-Host ""
Write-Host "완료: $zip"
Write-Host "SHA256: $hash"
