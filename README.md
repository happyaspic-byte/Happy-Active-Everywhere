# Everywhere — 개인용 P2P 동기화 개발 프로젝트

현재 결과물: **실험용 단일 파일 전송 CLI 알파**. macOS Apple Silicon에서 개발 중.
실제 사용자 데이터 적용은 후속 오류 주입·실장비 검증 후 판단한다.
Resilio Active Everywhere는 기능·성능 참고 기준이다. 비공개 프로토콜 호환성·성능 동등성을 주장하지 않는다.

## 구현 상태

| 영역 | 상태 |
|---|---|
| 장치별 인증서·TLS 1.3 상호 인증·명시적 승인/회수 | 구현, 로컬 CLI 테스트 |
| TCP 직접 전송·BLAKE3 검증·1 MiB 블록 | 구현, 로컬 CLI 테스트 |
| 강제 종료 후 재개·기존 파일 일치 블록 재사용 | 구현, SHA-256 회귀 |
| 임시 수신·검증·교체 전 버전 보존·오프라인 로컬 수정 보호 | 구현, 저장소 회귀 |
| 폴더 감시·SQLite·양방향·충돌·삭제 기록·복원 UI | 미구현 |
| 중앙 서버·인증 관리 화면·정책 배포 | 미구현 |
| OS 설치·자동 시작·업데이트·롤백·제거 패키지 | 미구현 |
| Windows·Ubuntu·Synology·실제 두 장비·WAN | 미검증 |
| 72시간·7일·100 GiB·대량 작은 파일·Resilio 비교 | 미검증 |

지원 범위·후속 데이터 모델: [docs/architecture.md](docs/architecture.md).
운영·복구·제한사항: [docs/alpha-operations.md](docs/alpha-operations.md).
실측·재현: [docs/verification.md](docs/verification.md).

## 빌드·시험

```sh
cargo build --release --locked
cargo test --locked --all-targets -- --nocapture
cargo clippy --locked --all-targets -- -D warnings
```

Rust 2024 edition. 최초 시험 환경 Cargo/rustc 1.97.1, macOS 26.6.2 arm64.
`Cargo.lock`으로 의존성을 고정한다.

## 격리된 로컬 두 장치 예시

다음 예시는 새 임시 디렉터리와 합성 파일만 사용한다. 인증서 신뢰 등록은 상대 장치의 파일 쓰기 권한을 부여하므로 실제 원격 장치에서는 별도의 신뢰된 경로로 인증서 fingerprint를 확인해야 한다.

```sh
BIN="$PWD/target/release/everywhere"
DEMO="$(mktemp -d)"
A="$DEMO/a"
B="$DEMO/b"
"$BIN" init --state "$A"
"$BIN" init --state "$B"
A_ID=$("$BIN" trust --state "$B" --cert "$A/identity.der")
B_ID=$("$BIN" trust --state "$A" --cert "$B/identity.der")
printf 'synthetic example\n' > "$DEMO/source"
"$BIN" receive --state "$B" --peer "$A_ID" \
  --listen 127.0.0.1:7443 --output "$DEMO/received" --once &
RECEIVER=$!
"$BIN" send --state "$A" --peer "$B_ID" \
  --addr 127.0.0.1:7443 --source "$DEMO/source"
wait "$RECEIVER"
shasum -a 256 "$DEMO/source" "$DEMO/received"
```

수신 `LISTEN` 출력 후 송신한다. 자동화된 시작 대기·전송·중단·재개는 `cargo test --test cli`에서 재현한다.
LAN/Tailscale에서는 명시적 IP/port로 수신 bind·송신 접속한다. 기본 bind는 loopback이다.
개인키 `identity.key.der`는 장치 밖으로 복사하지 않는다. 교환 대상은 공개 인증서 `identity.der`다.

## 로컬 성능 시험

```sh
python3 scripts/bench_transfer.py \
  --binary target/release/everywhere \
  --work-dir "$TMPDIR" --gib 10 \
  --report "$TMPDIR/everywhere-benchmark.json"
```

원본+대상+4 GiB 여유 공간 확인 후 합성 데이터를 생성한다. 생성한 시험 디렉터리는 종료 시 정리한다.
결과는 loopback, 캐시 미통제 조건이다. 실장비 LAN/WAN 성능 목표 달성 근거로 사용하지 않는다.
