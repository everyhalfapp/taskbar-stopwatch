# 보안 정책

보안 문제를 발견하셨다면 공개 이슈 대신 GitHub 의
[비공개 취약점 신고](https://github.com/everyhalfapp/taskbar-stopwatch/security/advisories/new)로 알려 주세요.

## 이 앱이 하는 일 / 하지 않는 일

- **네트워크를 전혀 쓰지 않습니다.** 서버 통신, 업데이트 확인, 사용 통계 수집이 없습니다.
- 관리자 권한을 요구하지 않습니다 (`asInvoker`).
- 파일은 `%APPDATA%\TaskbarStopwatch\` 의 `settings.ini`, `timer.ini` 만 씁니다.
- 레지스트리는 "Windows 시작 시 실행"을 켰을 때만
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\TaskbarStopwatch` 를 만들고, 끄면 지웁니다.
- 실행 파일은 직접 불러오는 DLL 을 System32 에서만 찾도록 빌드됩니다 (DLL 바꿔치기 방지).
