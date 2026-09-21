# CLI 알파 운영·복구

## 신뢰 경계
- 수신기는 `--peer`로 지정한 승인 장치와 `--output`으로 지정한 단일 경로만 허용한다.
- 서버는 클라이언트 인증서를 요구한다. 클라이언트도 서버 인증서를 검증한다.
- TLS 1.3 전용, ALPN `everywhere/1`, 인증서 DER BLAKE3 fingerprint 재확인.
- 인증서는 신뢰된 경로로 직접 교환한다. 자동 발견·등록·중앙 정책·폴더 ACL은 후속 기능이다.
- `revoke --state <state> --peer <fingerprint>`는 해당 상태 디렉터리의 승인을 삭제한다.
- 승인 상태는 연결 시·블록 경계·최종 반영 전 검사한다. 회수 전파는 로컬 장치 범위다.
- 수신기 한 프로세스당 전송 1개. TLS handshake 및 각 네트워크 I/O timeout 60초.
- 승인된 피어도 대량 파일을 보내 공간을 사용할 수 있다. quota·속도 제한은 후속 기능이다.

## 데이터 반영
수신자는 원격 파일 이름을 사용하지 않는다. 지정된 로컬 파일명만 처리한다.
대상 옆 `.everywhere-<path hash>.lock`으로 동일 대상 수신기를 직렬화한다.
블록은 전체 길이·BLAKE3 일치 후 `.part`에 쓰고 sync한다.
최초 대상 manifest를 `.json` 복구 저널에 저장한다. 재시작 중 대상 내용이 바뀌면 중단한다.
이어받기는 실제 `.part` 내용을 다시 읽어 검증한다. 손상 블록은 다시 요청한다.
최종 전체 manifest 검증 후 `.everywhere-versions/<path hash>-<old hash>`에 이전 내용 복사·검증·sync.
부분적으로 저장된 손상 버전은 `.corrupt-N`으로 보존하고 원본에서 재복사한다.
대상 수정 여부를 다시 확인하고 rename한 뒤 부모 디렉터리를 sync한다.

## 중단·오류 복구
- 네트워크 단절·프로세스 종료: 같은 state/peer/output/source로 수신기와 송신기를 다시 실행한다.
- `.part`·저널은 자동 삭제하지 않는다. 유효 블록만 재사용한다.
- 원본 변경: 현재 전송을 실패 처리한다. 변경 완료 후 새 manifest로 다시 전송한다.
- 로컬 대상 변경: 이전 전송의 재개를 거부한다. 원본·대상·부분 수신 데이터를 모두 보존한다.
- 저널 손상: 실패를 표시하고 대상 보존. 자동 저널 재작성은 하지 않는다.
- 공간 부족·버전 보존 실패: 기존 대상을 유지한다. 남아 있는 부분 데이터는 보존한다.
- `restore --version-file <보존 파일> --output <대상>`은 블록/전체 검증 후 복원한다. 현재 파일도 먼저 이전 버전으로 보존한다.

## 현재 한계
- 격리된 신뢰 가능한 로컬 폴더에서만 알파를 시험한다.
- 동시 수신기는 잠금으로 차단한다. 별도 프로그램이 마지막 검사와 rename 사이에 대상/상위 디렉터리를 바꾸는 경쟁은 완전히 차단하지 못한다.
- 심볼릭 링크는 검사 시 거부한다. 경로 기반 검사와 open 사이의 악의적인 로컬 교체를 방어하는 capability 기반 파일 I/O는 후속 보강 사항이다.
- macOS/Linux는 파일·디렉터리 sync. Windows 파일 교체·내구성 의미는 미검증이다.
- fsync는 OS·파일시스템 계약 범위다. 실제 전원 차단·스토리지 캐시 유실은 미시험이다.
- 파일 내용만 전송한다. 소유권·ACL·xattr·resource fork·희소 영역·hard link·mtime 보존은 미구현이다.
- 보존 버전·중단 파일의 자동 만료/정리는 없다. 디스크 사용량을 감시해야 한다.
- 현재 metadata manifest는 블록 수에 비례한다. payload 버퍼는 1 MiB, 단일 전송만 진행한다.
- 송신은 manifest 생성·각 요청 블록 검증·최종 재해시를 수행한다. 쓰기가 계속되는 파일은 중단될 수 있다.
- 네트워크 중단 시 이전 파일은 보존되지만 자동 재접속 daemon은 아직 없다.
- 로컬 인덱스는 폴더 marker·SQLite·삭제 기록을 사용한다. 네트워크 삭제 전파·자동 감시는 미구현이다.
- CLI 알파의 검증으로 전체 동기화 플랫폼 완료·성능 목표 달성을 선언하지 않는다.

## 로컬 폴더 인덱스

```sh
everywhere folder-init --db /isolated/state.sqlite --root /isolated/files --device device-a
everywhere scan --db /isolated/state.sqlite --root /isolated/files --device device-a
everywhere index-status --db /isolated/state.sqlite --root /isolated/files --device device-a
```

DB는 동기화 폴더 밖에 둔다. 폴더 경로·장치 ID·marker를 DB에 고정한다.
`scan`은 파일 해시를 재검사하고 SQLite 트랜잭션으로 버전 벡터를 증가시킨다.
누락 파일이 있으면 기본적으로 전체 인덱스 갱신을 거부한다.
`--allow-deletes`는 로컬 파일 누락을 명시적으로 삭제 기록에 반영하며 파일 자체를 삭제하지 않는다.
marker 누락·폴더 경로 변경·DB 손상 시 검사 중단, 기존 인덱스 보존.
symlink·특수 파일·UTF-8 외 이름·역슬래시 이름은 검사 실패 처리한다.
`.everywhere-` 접두사와 `.everywhere-folder`는 내부용 예약 이름으로 스캔에서 제외한다.
현재 스캔은 파일 목록을 메모리에 모으고 내용을 재해시한다. 대규모 시험·OS별 파일 이름 규칙 보강 필요.
DB 손상 자동 복구·원격 버전 적용·폴더 자동 감시는 아직 없다.
버전 벡터의 세 모드·충돌 판정은 라이브러리 테스트 범위이며 실제 양방향 동기화 완료를 의미하지 않는다.
