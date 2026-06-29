# gpx-motion

GPX 경로를 위에서 내려다본 지도 위에 애니메이션으로 그려 영상(MP4)·이미지(PNG)로 만드는 렌더러입니다. [GPX Animator](https://github.com/gpx-animator/gpx-animator)(Java)의 렌더링 모델을 Rust 코어로 포팅했습니다.

브라우저·screenshot·WebGL에 의존하지 않고 **CPU 메모리상의 픽셀 버퍼를 직접 2D 래스터 렌더링**(`tiny-skia`)한 뒤 rawvideo로 ffmpeg에 직결합니다.

## 특징

- GPX 파싱 → Web Mercator 좌표 변환 → `tiny-skia` 직접 래스터
- 시간 기반 보간(타임스탬프가 있으면 정지 구간이 그대로 표현), 없으면 거리 기반 등속
- 페이딩 tail + 현재 위치 마커
- TMS/XYZ 지도 타일 배경 (메모리 + 24h 디스크 캐시)
- pre-draw(전체 경로 사전 렌더), 복수 GPX 동시 렌더
- 카메라: 전체 트랙 자동 맞춤 또는 마커 추적(follow)
- 정보 오버레이(lat/lon/speed/time)와 attribution 텍스트
- rayon 프레임 병렬 렌더
- 동일 코어를 **WebAssembly**로 컴파일해 브라우저 canvas에서 직접 렌더

## 아키텍처

렌더 코어는 I/O가 전혀 없는 순수 라이브러리로, 네이티브 CLI와 브라우저가 함께 사용합니다.

| 크레이트      | 역할                                                                                                            |
| ------------- | --------------------------------------------------------------------------------------------------------------- |
| `crates/core` | GPX 파싱·좌표 변환·`tiny-skia` 래스터·텍스트 오버레이. **네트워크/인코딩/파일 I/O 없음** → `wasm32` 컴파일 가능 |
| `crates/cli`  | `clap` CLI + `reqwest`(타일 fetch) + ffmpeg(MP4) + `rayon`(병렬)                                                |
| `crates/wasm` | `wasm-bindgen` 바인딩 — 브라우저가 같은 코어로 프레임을 렌더                                                    |

타일 fetch와 영상 인코딩은 의도적으로 코어 밖에 둡니다. 네이티브는 `reqwest`+ffmpeg, 브라우저는 fetch+WebCodecs로 분기하되 렌더 코어는 공유됩니다.

## 요구사항

- Rust (stable, edition 2021)
- [ffmpeg](https://ffmpeg.org/) — MP4 인코딩(런타임에 외부 프로세스로 실행)
- 브라우저 데모용: [`wasm-pack`](https://rustwasm.github.io/wasm-pack/), pnpm

## 빌드

```sh
cargo build --release -p gpx-motion-cli
# 바이너리: target/release/gpx-motion
```

## CLI 사용법

출력 확장자가 모드를 결정합니다: `.mp4`는 애니메이션, 그 외는 PNG 한 장.

```sh
# 트랙 PNG 한 장
gpx-motion --input samples/seoul.gpx --output track.png

# 지도 배경 위 애니메이션 MP4
gpx-motion --input samples/seoul.gpx --output movie.mp4 \
  --total-time 8000 --fps 30 \
  --tms-url-template 'https://tile.openstreetmap.org/{zoom}/{x}/{y}.png'

# 마커를 따라가는 카메라(고정 zoom)
gpx-motion --input samples/seoul.gpx --output follow.mp4 --total-time 6000 --zoom 16

# 여러 트랙 동시 + 전체 경로 사전 렌더
gpx-motion --input a.gpx b.gpx --output multi.mp4 --total-time 8000 --pre-draw-track
```

### 주요 옵션

| 옵션                          | 기본값                          | 설명                                                 |
| ----------------------------- | ------------------------------- | ---------------------------------------------------- |
| `--input <FILE>...`           | (필수)                          | GPX 입력. 여러 개 지정 시 동시 렌더                  |
| `--output <FILE>`             | `track.png`                     | `.mp4`면 애니메이션, 그 외 PNG                       |
| `--viewport-width/height`     | `640`                           | 뷰포트 크기                                          |
| `--fps`                       | `30`                            | 프레임레이트                                         |
| `--total-time <MS>`           | 트랙 실제 시간                  | 영상 길이(ms)                                        |
| `--tail-duration <MS>`        | `5000`                          | 꼬리 길이(ms)                                        |
| `--tms-url-template <URL>`    | (없음)                          | 타일 URL(`{zoom}/{z}`,`{x}`,`{y}`). 없으면 단색 배경 |
| `--background-map-visibility` | `1.0`                           | 지도 불투명도                                        |
| `--pre-draw-track`            | off                             | 전체 경로를 흐리게 사전 렌더                         |
| `--pre-draw-track-color`      | `#808080`                       | 사전 렌더 색                                         |
| `--zoom <Z>`                  | (없음)                          | 지정 시 마커 추적 카메라                             |
| `--information-position`      | `bottom left`                   | 정보 오버레이 위치(또는 `hidden`)                    |
| `--attribution`               | `© OpenStreetMap contributors`  | attribution 텍스트                                   |
| `--attribution-position`      | `bottom right`                  | attribution 위치(또는 `hidden`)                      |
| `--jobs <N>`                  | 전체 코어                       | 병렬 렌더 스레드                                     |
| `--no-tile-cache`             | off                             | 디스크 타일 캐시 비활성화                            |
| `--tile-cache-dir <DIR>`      | `$HOME/.cache/gpx-motion/tiles` | 타일 캐시 경로                                       |

> 타일 제공자의 사용 정책을 반드시 준수하세요. 기본 타일 소스는 없으며, `--tms-url-template`을 직접 지정해야 지도 배경이 그려집니다.

## 브라우저(WASM) 데모

같은 렌더 코어가 WebAssembly로 컴파일되어 canvas에서 트랙을 애니메이션합니다.

```sh
pnpm install
pnpm dev      # build:wasm(wasm-pack) 후 Vite dev 서버 → http://localhost:5173
```

`pnpm build`도 wasm 빌드를 선행한 뒤 프로덕션 번들을 만듭니다. 생성된 `crates/wasm/pkg/`는 빌드 산출물입니다.

## 개발

```sh
cargo test                              # 단위 테스트
cargo fmt --all                         # 포맷
cargo clippy --all-targets -- -D warnings
cargo build -p gpx-motion-wasm --target wasm32-unknown-unknown   # wasm 회귀 확인
```

CI(GitHub Actions)는 push/PR에서 fmt·clippy·test·wasm 빌드를 실행하고, `v*` 태그에서 Linux/macOS/Windows 단일 바이너리를 릴리스합니다.

## 라이선스 / 출처

- 원본 모델: [gpx-animator/gpx-animator](https://github.com/gpx-animator/gpx-animator) (Apache-2.0)
- 번들 폰트: RobotoMono (SIL Open Font License — `crates/core/assets/font-OFL.txt`)
- 지도 타일: 사용하는 타일 제공자의 라이선스·attribution을 따르세요(OSM 등)
