# Happy-Active-Everywhere — 개인용 P2P 동기화 개발 프로젝트

현재 결과물: **양방향 폴더 동기화와 로컬 관리 화면을 갖춘 개발용 알파**.
GitHub Actions에서 Windows·macOS·Ubuntu를 시험합니다. 실장비·NAS·실제 WAN·장시간 안정성 검증과 배포 작업은 진행 중입니다.
Resilio Active Everywhere는 참고 기준이며, 프로토콜 호환성이나 성능 동등성을 주장하지 않습니다.

## 시작하기

[폴더 동기화 안내](docs/folder-quickstart.md)에 따라 장치 초기화, 인증서 교환,
폴더 승인, 수신과 연결을 설정하세요. 상태 디렉터리는 동기화 폴더 밖에 둡니다.

```sh
everywhere management-token --state /path/to/device-state
everywhere manage --state /path/to/device-state --listen 127.0.0.1:7445
```

브라우저에서 `http://127.0.0.1:7445`에 접속해 표시한 관리 토큰으로 로그인합니다.
이 화면은 해당 컴퓨터만 관리하며 기본적으로 외부 네트워크에 노출하지 않습니다.
토큰은 URL이나 HTML에 포함되지 않습니다. 기기의 개인키는 다른 장비에 복사하지 않습니다.

## 구현 상태

| 영역 | 상태 |
|---|---|
| TLS 1.3 상호 인증·장치 및 폴더별 승인·권한 회수 | 구현·회귀 시험 |
| BLAKE3 블록 검증·재개·최대 16블록 전송 파이프라인 | 구현·실제 프로세스 시험 |
| 검증 후 반영·열린 파일의 늦은 수정 보존·중단 복구 | 구현·회귀 시험 |
| SQLite 인덱스·마운트 확인·지속되는 삭제 승인 대기 | 구현·회귀 시험 |
| 양방향 폴더 동기화·세 장비 오프라인 충돌 수렴 | 구현·세 OS 시험 |
| 충돌 선택·로컬 이전 버전 복원·복원 내용 재전파 | 구현·실제 TLS 시험 |
| 로컬 관리 화면·토큰 인증·개별 삭제 검토·복구 | 구현·HTTP 및 실제 브라우저 시험 |
| 백그라운드 작업·설치·업데이트·롤백 | 개발 중 |
| 중앙 다중 사용자 관리·릴레이·NAT 자동 연결 | 제공하지 않음 |
| 이벤트 감시·대량 폴더 최적화·NAS·실제 LAN/WAN·장시간 시험 | 미완료 |

기능 시험 통과와 완제품 인증을 구분합니다. 최신 커밋의 서식·Clippy·전체 시험·브라우저·release 빌드가 모두 통과하기 전에는 배포 후보로 사용하지 않습니다.
[작업 및 검증 기록](docs/product-work.md)에서 실패한 회귀 시험과 수정 결과를 확인할 수 있습니다.
[폴더 설계](docs/folder-sync-design.md), [관리 설계](docs/management-design.md),
[기존 전송 알파 운영](docs/alpha-operations.md), [초기 실측](docs/verification.md)도 참고하세요.
초기 단일 파일·인덱싱 실측은 새 폴더 동기화 성능을 입증하지 않습니다.

## 빌드·시험

```sh
cargo build --release --locked
cargo test --locked --all-targets -- --nocapture
cargo clippy --locked --all-targets -- -D warnings
```

Rust 2024 edition. CI는 stable Rust를 사용하며 실제 버전을 로그에 기록합니다. 최초 로컬 실측 환경과 현재 hosted runner 환경은 다릅니다.
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
