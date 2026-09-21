# 검증 기록 — 2026-09-21

## 환경
macOS 26.6.2 (25G83), Apple Silicon arm64, 8코어, RAM 8 GiB.
Cargo/rustc 1.97.1. 빈 저장소에서 feature/safe-transfer-alpha 브랜치 시작.
타 장비 정확한 OS/NAS 모델 및 실행 권한 미확인. Grok 4.6 하위 작업 2건은 인증 401로 실행 불가.
독립 하위 에이전트 리뷰는 수행되지 않았으며 직접 검사·자동화 테스트 결과만 기록한다.

## 실패 우선 기록
- CLI stub에서 장치 인증서 생성과 서버 시작 테스트 2건 실패 후 구현.
- 저장소 stub에서 검증·재개·재사용·빈 파일·symlink 테스트 4건 실패 후 구현.
- 재시작 사이 로컬 대상 변경 보호 테스트 실제 실패 후 최초 대상 상태 저널로 수정.
- 중단된 이전 버전 복사의 재시도 테스트 실제 실패 후 손상 버전 격리·재복사로 수정.
- 처음 CLI 테스트의 수신 stdout pipe를 조기 닫는 테스트 결함을 수정하고 전체 재실행.

## 자동화 결과
`cargo test --locked --all-targets -- --nocapture`: CLI 6개 + 저장소 8개 통과.

- CLI: 정상 전송 2회, 송신·수신 강제 종료 후 재개 2회, SHA-256 동일.
- 재개 2회 모두 검증된 블록 1개 재사용, 나머지 5개 전송.
- 상호 인증·미승인 피어 거부·로컬 권한 회수·전송 중 권한 회수.
- 전송 중 원본 변경과 실제 파일 권한 오류에서 기존 대상 유지.
- 잘못된 블록·부분 데이터 손상·저널 손상·이전 버전 부분 복사.
- 일치 블록 재사용·동일 대상 수신 잠금·로컬 수정·빈 파일·잘못된 manifest·symlink.

`cargo clippy --locked --all-targets -- -D warnings`: 통과.
`cargo build --release`: 통과.

## 10 GiB 실측
재현:

```sh
cargo build --release --locked
python3 scripts/bench_transfer.py --binary target/release/everywhere \
  --work-dir "$TMPDIR" --gib 10 --report "$TMPDIR/everywhere-10gib.json"
```

[기계 판독 결과](benchmarks/macos-loopback-10gib.json).

| 항목 | 실측 |
|---|---:|
| 데이터 크기 | 10,737,418,240 bytes |
| end-to-end 시간 (manifest·재해시 포함) | 130.5816초 |
| 전송 처리량 | 78.4184 MiB/s |
| 송신 프로세스 최대 RSS | 6,553,600 bytes (6.25 MiB) |
| 합성 원본 쓰기+fsync | 4.8365초 |
| 원본 독립 SHA-256 계산 | 5.8826초 |
| 독립 SHA-256 불일치 | 0 |

SHA-256: `91f2ba191af80a72e75595bb4f4b1e5f50a6e0368b356f289829c9b31c382b45`.
수신 10,240블록. 실제 release CLI·TLS 1.3 경로를 통과했다.
조건: 같은 호스트·같은 파일시스템·loopback·캐시 미통제, source는 사전 SHA-256 읽기.
이 수치로 네트워크/디스크 상한이나 80% 목표 달성을 판정하지 않는다.
수신 RSS·전체 에이전트 유휴 RSS·파일 감시 지연 p95는 미측정.
현재 블록별 fsync·왕복 ACK를 사용하므로 WAN 효율은 추가 측정·개선이 필요하다.

## 미검증·미구현
- 실제 두 장비, Windows, Ubuntu, Synology, LAN/WAN, 3~10장치.
- ENOSPC, 마운트 해제, 실제 전원 차단, OS별 원자 교체·서비스 수명주기.
- 100 GiB: 현재 여유 공간 약 96 GiB로 원본·대상 확보 불가.
- 100,000·1,000,000파일 인덱스, SQLite 복구, 동기화 모드·충돌·삭제.
- 관리 API·웹 화면, 설치·업데이트·롤백·제거, 72시간·7일 시험.
- Resilio Sync 비교 및 Active Everywhere 직접 측정.

원본 실행 로그는 목표 세션 전용 scratch에 `inventory.log`, `cli-red.log`,
`storage-red.log`, `resume-red.log`, `version-red.log`, `alpha-final-tests.log`,
`release-build.log`, `clippy-final.log`, `benchmark-10gib.json`으로 저장했다.
본 문서와 저장소 테스트·실측 JSON은 지속 보존용 결과다.
