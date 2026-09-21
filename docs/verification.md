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
`cargo test --locked --all-targets -- --nocapture`: CLI 9개 + 저장소 8개 + 인덱스 4개 + 버전 판단 3개 = 24개 통과.

- CLI: 정상 전송 2회, 송신·수신 강제 종료 후 재개 2회, SHA-256 동일.
- 재개 2회 모두 검증된 블록 1개 재사용, 나머지 5개 전송.
- 상호 인증·미승인 피어 거부·로컬 권한 회수·전송 중 권한 회수.
- 전송 중 원본 변경과 실제 파일 권한 오류에서 기존 대상 유지.
- 잘못된 블록·부분 데이터 손상·저널 손상·이전 버전 부분 복사.
- 일치 블록 재사용·동일 대상 수신 잠금·로컬 수정·빈 파일·잘못된 manifest·symlink.
- CLI 폴더 등록·반복 재검사·삭제 승인·상태 영속화·현재 버전 보존 복원.
- 폴더 marker 소실·경로 변경·DB 손상에서 데이터 보존.
- 벡터 인과관계·삭제/수정 동시 충돌·세 모드 판정 (라이브러리 범위).
- 역슬래시 파일명과 중첩 경로의 별칭 충돌 실패를 확인하고 스캔 거부로 수정.

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
- 100,000·1,000,000파일 전송, SQLite 자동 복구, 실제 동기화 모드·충돌·삭제 전파.
- 관리 API·웹 화면, 설치·업데이트·롤백·제거, 72시간·7일 시험.
- Resilio Sync 비교 및 Active Everywhere 직접 측정.

원본 실행 로그는 목표 세션 전용 scratch에 `inventory.log`, `cli-red.log`,
`storage-red.log`, `resume-red.log`, `version-red.log`, `alpha-final-tests.log`,
`release-build.log`, `clippy-final.log`, `benchmark-10gib.json`으로 저장했다.
본 문서와 저장소 테스트·실측 JSON은 지속 보존용 결과다.

인덱스·복원 추가 검증 로그: `versions-red.log`, `versions-green.log`, `index-red.log`, `index-cli-red.log`, `index-path-red.log`, `restore-red.log`, `index-restore-tests.log`, `index-clippy.log`.

## 10만 작은 파일 로컬 인덱스 실측

`python3 scripts/bench_index.py --binary target/release/everywhere --work-dir "$TMPDIR" --files 100000 --report "$TMPDIR/everywhere-index-100k.json"`

- 최초 스캔 10.5116초, 변경 없는 전체 재검사 10.3046초.
- 최초 스캔 프로세스 최대 RSS 35,880,960 bytes (34.22 MiB).
- 10만 개 가변 길이 ASCII 합성 파일, 최근 생성된 캐시 미통제 조건.
- 실제 `folder-init`·`scan` CLI 경로 실행. 독립 경로 목록+SHA-256 집계로 누락 0·내용 불일치 0.
- 원본·검사 후 집계 SHA-256: `64d510240464bb4bac886b0c3b6f167933f462c1489393e8617e9cefb97d1fb3`.
- 이 결과는 로컬 인덱스 측정이다. 작은 파일의 네트워크 동기화와 상주 에이전트 유휴 RSS는 미검증.
- [실측 JSON](benchmarks/macos-index-100k.json).

주기 감시 CLI: 파일 생성 감지·누락 오류·재생성 후 수정 감지·프로세스 재시작 상태 보존 통과. `watch-red.log`에 누락 진입점 실패, `watch-green.log`·`watch-clippy.log`에 전체 회귀/정적 검사 결과 보존.

## 100만 작은 파일 로컬 인덱스 실측

`python3 scripts/bench_index.py --binary target/release/everywhere --work-dir "$TMPDIR" --files 1000000 --report "$TMPDIR/everywhere-index-1m.json"`

- 최초 스캔 120.3834초, 변경 없는 전체 재검사 124.4487초.
- 최초 스캔 최대 RSS 262,602,752 bytes (250.44 MiB).
- 100만 합성 파일의 독립 목록·SHA-256 비교: 누락 0, 내용 불일치 0.
- 집계 SHA-256: `231768af19e5b28376d81b554dde84eddcd5b386ddc446df8f54101ed2e29d83`.
- 로컬 인덱스 실제 CLI 측정. 네트워크 전송·상주 유휴 RSS·LAN 반영 지연은 미측정.
- [실측 JSON](benchmarks/macos-index-1m.json).

최종 재검증: `final-release.log`·`final-tests.log`·`final-clippy.log`에서 release build, 24개 테스트, 형식 검사, Clippy 경고 0 확인.
