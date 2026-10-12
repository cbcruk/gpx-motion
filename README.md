# gpx-motion

GPX 경로를 위에서 내려다본 지도 위에 애니메이션으로 그려 영상(MP4)·이미지(PNG)로 만드는 렌더러입니다. [GPX Animator](https://github.com/gpx-animator/gpx-animator)(Java)의 렌더링 모델을 Rust 코어로 포팅했습니다.

브라우저·screenshot·WebGL에 의존하지 않고 **CPU 메모리상의 픽셀 버퍼를 직접 2D 래스터 렌더링**(`tiny-skia`)한 뒤 rawvideo로 ffmpeg에 직결합니다.

## 특징

- GPX 파싱 → Web Mercator 좌표 변환 → `tiny-skia` 직접 래스터
- 시간 기반 보간(타임스탬프가 있으면 정지 구간이 그대로 표현), 없으면 거리 기반 등속
- 페이딩 tail + 현재 위치 마커
- 고도 표현: 고도별 트랙 색상, 하단 고도 프로필 그래프, 고도·누적 상승/하강 수치
- 3D 원근 시점: 트랙을 고도만큼 들어 올리고 지면까지 커튼을 드리움, 카메라 궤도 회전
- 라벨 없는 벡터 지도(MVT, OpenMapTiles 스키마)를 코어에서 직접 렌더 — `light`/`dark` 스타일, 소스 maxzoom을 넘는 확대에서도 선명
- TMS/XYZ 래스터 지도 타일 배경
- 타일 메모리 + 24h 디스크 캐시
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
| `crates/wasm` | `wasm-bindgen` 바인딩 — 브라우저 2D는 같은 코어로 렌더, 3D는 코어가 계산한 데이터를 three.js(WebGL)가 그림      |

타일 fetch와 영상 인코딩은 의도적으로 코어 밖에 둡니다. 네이티브는 `reqwest`+ffmpeg, 브라우저는 fetch+WebCodecs로 분기하되 렌더 코어는 공유됩니다.

## 요구사항

- Rust 1.88 이상(개발·CI는 `rust-toolchain.toml`에 고정된 버전, wasm32 타깃 포함)
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

# 벡터 지도(라벨 없음) 위 애니메이션 MP4
gpx-motion --input samples/seoul.gpx --output movie.mp4 \
  --total-time 8000 --fps 30 \
  --vector-tiles https://tiles.openfreemap.org/planet --map-style dark

# 래스터 지도 타일 배경
gpx-motion --input samples/seoul.gpx --output movie.mp4 \
  --total-time 8000 --fps 30 \
  --tms-url-template 'https://tile.openstreetmap.org/{zoom}/{x}/{y}.png'

# 고도 표현: 색상 + 프로필 그래프
gpx-motion --input samples/namsan.gpx --output hike.mp4 --total-time 8000 \
  --color-by-elevation --elevation-profile \
  --vector-tiles https://tiles.openfreemap.org/planet --map-style dark

# 3D 원근 시점 + 궤도 회전
gpx-motion --input samples/namsan.gpx --output hike-3d.mp4 --total-time 8000 \
  --pitch 55 --bearing 20 --orbit 60 --color-by-elevation --elevation-profile \
  --vector-tiles https://tiles.openfreemap.org/planet --map-style dark

# 마커를 따라가는 카메라(고정 zoom)
gpx-motion --input samples/seoul.gpx --output follow.mp4 --total-time 6000 --zoom 16

# 전체 경로에서 시작해 마커로 줌 인, 끝에서 다시 줌 아웃(2초씩)
gpx-motion --input samples/namsan.gpx --output zoom.mp4 --total-time 8000 \
  --zoom 17 --zoom-transition 2 --vector-tiles https://tiles.openfreemap.org/planet --map-style dark

# 여러 트랙 동시 + 전체 경로 사전 렌더
gpx-motion --input a.gpx b.gpx --output multi.mp4 --total-time 8000 --pre-draw-track
```

### 주요 옵션

| 옵션                          | 기본값                          | 설명                                                                                                                          |
| ----------------------------- | ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `--input <FILE>...`           | (필수)                          | GPX 입력. 여러 개 지정 시 동시 렌더                                                                                           |
| `--output <FILE>`             | `track.png`                     | `.mp4`면 애니메이션, 그 외 PNG                                                                                                |
| `--viewport-width/height`     | `640`                           | 뷰포트 크기(`1..=16384`)                                                                                                      |
| `--fps`                       | `30`                            | 프레임레이트(MP4, 양수)                                                                                                       |
| `--total-time <MS>`           | 트랙 실제 시간                  | 영상 길이(ms, 양수)                                                                                                           |
| `--tail-duration <MS>`        | 2D `5000`, 3D 전체              | 꼬리 길이(ms, MP4), 시작 쪽으로 갈수록 흐려짐                                                                                 |
| `--vector-tiles <URL>`        | (없음)                          | 벡터 타일 TileJSON URL, `{z}/{x}/{y}.pbf` 템플릿, 또는 `.pmtiles`(로컬 경로나 HTTP Range를 지원하는 URL, OpenMapTiles 스키마) |
| `--vector-max-zoom <Z>`       | `14`                            | XYZ 벡터 템플릿의 최대 zoom(TileJSON은 자동)                                                                                  |
| `--map-style`                 | `light`                         | 벡터 지도 스타일: `light`, `dark`, 또는 JSON 스타일 파일 경로·URL(자체 형식이나 MapLibre 스타일, 아래)                        |
| `--tms-url-template <URL>`    | (없음)                          | 래스터 타일 URL(`{zoom}/{z}`,`{x}`,`{y}`)                                                                                     |
| `--background-map-visibility` | `1.0`                           | 지도 불투명도                                                                                                                 |
| `--pre-draw-track`            | off                             | 전체 경로를 흐리게 사전 렌더(MP4)                                                                                             |
| `--pre-draw-track-color`      | `#808080`                       | 사전 렌더 색(`#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`)                                                                         |
| `--zoom <Z>`                  | (없음)                          | 지정 시 마커 추적 카메라(`0..=22`, 소수 가능, 예: `15.5`)                                                                     |
| `--zoom-transition <S>`       | (없음)                          | 영상 시작에 전체 경로→`--zoom`으로 S초 줌 인, 끝에 다시 줌 아웃(MP4 전용, 2D·3D, `--zoom` 필요)                               |
| `--color-by-elevation`        | off                             | 트랙을 고도에 따라 색칠(낮음 파랑 → 높음 빨강)                                                                                |
| `--elevation-profile`         | off                             | 하단 거리-고도 그래프(MP4)                                                                                                    |
| `--pitch <DEG>`               | `0`                             | 0보다 크면 3D 원근 시점(`0..=75`)                                                                                             |
| `--bearing <DEG>`             | `0`                             | 3D 화면 위쪽 방위                                                                                                             |
| `--orbit <DEG>`               | `0`                             | 영상 동안 카메라가 회전할 각도                                                                                                |
| `--follow-heading`            | 꺼짐                            | 3D 카메라가 첫 트랙의 진행 방향을 따라 회전(`--bearing`은 오프셋)                                                             |
| `--heading-window <M>`        | 영상 2초 이동 거리(최소 150m)   | 진행 방향을 평균할 트랙 거리                                                                                                  |
| `--max-turn-rate <DEG/S>`     | `90`                            | 추적 카메라 최대 회전 속도(영상 1초당, 양수)                                                                                  |
| `--terrain-tiles <TEMPLATE>`  | 없음                            | Terrarium 고도 타일(XYZ PNG)로 3D 지면 기복 표현                                                                              |
| `--labels`                    | off                             | 벡터 지도의 지명·산·수역 이름 표시(트랙·오버레이를 피해 중요도순 배치)                                                        |
| `--label-font <PATH>`         | 시스템 한글 폰트                | 라벨 폰트(TTF/OTF, `.ttc`는 첫 face)                                                                                          |
| `--label-language <CODE>`     | 현지 이름                       | `name:<code>`가 있으면 그 이름(예: `ko`, `en`)                                                                                |
| `--label-size <PX>`           | `13`                            | 라벨 기본 크기(CSS px, `4..=96`, 도시는 크게·동네는 작게)                                                                     |
| `--pixel-ratio <F>`           | `1`                             | 화면 배율(`0.25..=8`): 선 폭·마커·텍스트·여백을 함께 키움(예: 2배 해상도 출력에 `2`)                                          |
| `--fov <DEG>`                 | `40`                            | 3D 세로 시야각(`10..=120`)                                                                                                    |
| `--elevation-exaggeration`    | 자동(1–6×)                      | 3D 높이 배율                                                                                                                  |
| `--elevation-smoothing <M>`   | `100`                           | 고도 평활화 창(트랙 거리 m), `0`이면 기록값 그대로                                                                            |
| `--information-position`      | `bottom left`                   | 정보 오버레이 위치: `top left`, `top right`, `bottom left`, `bottom right`, `hidden`                                          |
| `--attribution`               | 지도 소스에 맞춘 OSM 표기       | attribution 텍스트                                                                                                            |
| `--attribution-position`      | `bottom right`                  | attribution 위치(`--information-position`과 같은 값)                                                                          |
| `--jobs <N>`                  | 전체 코어                       | 병렬 렌더 스레드                                                                                                              |
| `--no-tile-cache`             | off                             | 디스크 타일 캐시 비활성화                                                                                                     |
| `--tile-cache-dir <DIR>`      | `$HOME/.cache/gpx-motion/tiles` | 타일 캐시 경로                                                                                                                |

3D 시점은 벡터 지도(또는 지도 없음)에서만 동작합니다. 래스터 타일은 원근 변형할 수 없습니다. `--terrain-tiles`를 주면 경로 주변 지면이 실제 지형(DEM)만큼 솟고 음영 기복이 입혀지며, 트랙은 지형 위에 놓입니다(예: AWS Terrain Tiles `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png`). 지형은 경로 주변 영역에만 적용되고 바깥은 평면 지도로 이어지며, 산 뒤로 넘어간 트랙과 마커는 픽셀 단위로 가려져 흐린 선과 점으로만 비쳐 보이고(커튼은 그리지 않음)(`--zoom` 추적 카메라는 언덕이 마커를 가릴 구간을 앞두고 3초에 걸쳐 천천히 더 내려다보기 시작해 1초 전에 다 기울고, 지나간 뒤 1초 머물렀다 2초에 걸쳐 돌아옴), 점이 성긴 구간은 지형을 따라 잘게 나눠 언덕을 관통하지 않습니다. 브라우저 데모도 같은 지형을 WebGL로 그립니다(지도 텍스처는 워커가 그림, 2D 지도도 워커가 그려 로드 중 화면이 멈추지 않음). 3D는 기본으로 시작부터 현재까지의 경로 전체를 그리고, `--tail-duration`을 주면 2D처럼 흐려지는 꼬리로 그립니다. 3D 지도의 도로는 멀수록 가늘게 그립니다.

`--zoom-transition`은 영상 앞뒤 S초 동안 전체 경로 화면과 `--zoom` 추적 화면 사이를 한 번의 줌처럼 오갑니다. 줌하는 동안 벡터 지도의 타일 zoom이 바뀌는 지점에서는 두 zoom의 지도를 0.3 zoom에 걸쳐 교차 페이드해 세부가 한 프레임에 바뀌지 않습니다(3D는 먼 화면의 타일 수 한도까지 반영). 3D에서 `--terrain-tiles`와 함께 쓰면 경로 주변의 추적 zoom 지형에 더해, 전체 경로 화면 zoom의 성긴 지형·지도가 경로 크기만큼 더 넓게 둘러싸 줌 아웃 구간에도 기복이 화면 멀리까지 이어지고, 지형 텍스처는 밉맵으로 축소해 멀리서도 반짝이지 않습니다(넓은 지형 텍스처를 그리는 만큼 준비 시간이 늘어남).

`--labels`는 `place`, `mountain_peak`, `water_name` 레이어의 점 라벨과, `transportation_name`·`waterway`의 도로명·하천명을 선을 따라 그립니다(글자마다 선 방향으로 돌리고, 거꾸로 읽히지 않게 뒤집으며, 급하게 꺾이는 곳은 피하고, 긴 선에는 간격을 두고 반복; 작은 도로 이름은 가까이 확대했을 때만). 한글 등 지명 문자를 가진 폰트가 필요하며, `--label-font`가 없으면 시스템 폰트(macOS Apple SD Gothic Neo, Linux Noto Sans CJK/나눔고딕, Windows 맑은 고딕)를 찾습니다. 폰트에 글리프가 없는 이름은 건너뜁니다. 3D에서는 멀수록 작고 흐려지며, 트랙 커튼 위에는 놓지 않습니다. 영상에서는 직전 프레임에 보이던 라벨을 먼저 배치해 카메라가 움직여도 자리를 지키고, 라벨은 0.4초에 걸쳐 나타나고 사라지며, 지형 뒤로 가린 라벨은 흐려지다 빠집니다. 셰이핑이 필요한 문자(아랍어·히브리어·인도계 등)는 rustybuzz로 셰이핑해 이어 쓰고 오른쪽→왼쪽 순서로 그리며(선을 따르는 이름은 거의 곧은 구간에만), 폰트에 글리프가 없으면 라틴 표기(`name:latin`)로 대신합니다. 화면 밖으로 나가는 라벨은 가장자리에 걸친 채 흐려지며 사라지고, 정보 표시나 트랙 아래로 들어가는 도로명·하천명은 가려진 글자를 빼고 흐려집니다. 브라우저 데모도 같은 라벨을 그리되(출처 표기 아래로는 놓지 않음), wasm 크기(gzip +200KB) 때문에 셰이핑은 빼고 그런 이름은 라틴 표기로 대신하며(core의 `shaping` 기능), 폰트(NanumGothic)는 jsDelivr에서 받아 옵니다.

### 지도 스타일 파일

`--map-style`에 JSON 파일을 주면 색과 레이어 순서를 직접 정할 수 있습니다(예시: `styles/minimal.json`, `styles/blueprint.json`, `styles/dusk.json`). 브라우저에서는 같은 JSON 문자열을 `renderMap` / `setMapStyle`에 넘깁니다.

프리셋을 이어받아 배경만 바꾸려면:

```json
{ "extends": "dark", "background": "#231a2e" }
```

규칙을 직접 정의하려면:

```json
{
  "background": "#0f2a44",
  "rules": [
    { "layer": "water", "fill": "#0b2238" },
    { "layer": "transportation", "classes": ["primary", "trunk"], "line": "#9cc4e8", "width": 3 }
  ]
}
```

- `extends`(선택): 프리셋(`light`, `dark`)의 배경과 규칙에서 시작
- `background`: 불투명 배경색. `extends`가 없으면 필수
- `rules`: 있으면 프리셋 규칙을 **모두** 대체(이어 붙이지 않음). 위에서부터 차례로 그림
  - `layer`: OpenMapTiles 레이어, `classes`(선택): 해당 `class` 값만
  - `fill`: 레이어의 면(폴리곤) 피처를 채움
  - `line` + `width`: 레이어의 선 피처를 그림. 폭은 zoom 14 기준 CSS 픽셀(`0 < width ≤ 100`)
  - 선 규칙의 `width`는 zoom 곡선(MapLibre `interpolate`/`step` 또는 `{"stops": [[z, w], …], "base": b}`, MapLibre zoom 기준 CSS 픽셀)일 수도 있고, 이때는 기본 zoom 배율 대신 곡선을 따름
  - (선택) `opacity`: 투명도(수치 또는 zoom 곡선), `colors`: zoom에 따라 바뀌는 색(`{"stops": [[z, "#rrggbb"], …], "base": b}` 또는 MapLibre `interpolate`·`step`, MapLibre zoom 기준, `fill`/`line` 색 대신), `outlines`: 선 규칙이 면의 외곽선도 그림, `cap`: `butt`/`round`/`square`, `join`: `miter`/`round`/`bevel`(선 규칙, 기본 round), `blur`·`gap`: 선 가장자리 흐림·가운데 틈 폭(선 규칙, CSS px, 수치 또는 zoom 곡선), `antialias`: `false`면 면 가장자리를 계단으로(또는 `["step", ["zoom"], false, z, true]`), `translate`: 그리는 것을 `[x, y]` CSS px만큼 옮김(또는 zoom 곡선), `extrude`: 3D 보기에서 면을 건물처럼 세움(면 규칙, `{"height": 높이, "base": 바닥 높이, "gradient": 아래쪽 벽을 어둡게(기본 true)}`, 높이는 미터 수치 또는 속성 이름 예: `"render_height"`; 2D에서는 평면 채우기), 최상위 `"tiles": "maplibre"`: MapLibre 데이터 zoom 사용
- (선택) `filter`: MapLibre 필터(`==`, `!=`, `<`…, `in`, `has`, `match`, `all`/`any`/`none`/`!`, `["get", …]`, `["geometry-type"]`), `dash`: 선 폭 배수의 점선 패턴, `minzoom`/`maxzoom`: MapLibre zoom 범위
- 색상: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` (`colors`의 단계 값은 MapLibre처럼 CSS 색 문자열도 받음)
- 알 수 없는 키는 오류로 처리해 오타를 바로 알려 줍니다.

**MapLibre 스타일 가져오기**: OpenMapTiles 데이터용 MapLibre(Mapbox GL) 스타일 JSON을 그대로 줄 수 있습니다(파일 또는 URL, 예: `--map-style https://tiles.openfreemap.org/styles/liberty`). `background`·`fill`·`line` 레이어의 필터, `minzoom`/`maxzoom`, zoom에 따른 색(`step` 포함)·투명도·선 폭, 점선, 선 끝·모서리 모양(`line-cap`·`line-join`), 선 흐림(`line-blur`, 같은 양만큼 옅게 그리는 근사)·가운데 틈(`line-gap-width`), 면의 외곽선 색(`fill-outline-color`, 안티앨리어싱을 끄지 않은 면에 1 CSS px 선)·안티앨리어싱(`fill-antialias`)·오프셋(`fill-translate`, 지도 축 기준)을 옮기고, 선 레이어는 면의 외곽선도 그립니다. 3D 건물(`fill-extrusion`)은 높이·바닥 높이(수치, `["get", 속성]`)와 아래쪽 벽 어둡게(`fill-extrusion-vertical-gradient`)를 옮겨 3D 보기(CLI와 브라우저 WebGL)에서 건물로 세웁니다(지형 위에 서고, 언덕 뒤에 가려지고, 건물 뒤의 트랙은 지형 뒤처럼 흐린 가는 선으로; 조명은 MapLibre 기본 방향(남남서 위에서)을 화면이 아니라 지도에 고정해 회전해도 같은 벽이 밝음, 레이어 순서와 상관없이 지면 위에 그림; 추적 카메라의 마커 시야·최소 고도는 지형만 고려해 마커가 높은 건물 뒤에서 흐리게 보일 수 있고, 브라우저는 건물 뒤의 마커 점을 가리지 않음). 2D 보기에서는 평면 채우기로 그립니다. 타일은 MapLibre와 같은 데이터 zoom에서 가져옵니다(CLI와 브라우저 2D; 브라우저 3D 지면은 512px 타일이라 원래 같음). 패턴 채우기·선(`fill-pattern`·`line-pattern`)은 스타일의 스프라이트에서 그 패턴의 평균 색을 구해 그 색으로 그립니다(CLI; 스프라이트에 없는 패턴은 MapLibre처럼 그리지 않고, 브라우저의 `mapStyleNotes`는 스프라이트를 읽지 않으므로 건너뛴다고 알림). 라벨(`symbol`), 래스터, 데이터에 따라 바뀌는 값 등 옮길 수 없는 부분은 종류별 경고로 알려 주고 건너뜁니다. `--export-style out.json`은 가져온 결과를 위의 자체 형식으로 저장합니다(확인·수정용). 브라우저에서는 `mapStyleNotes(style)`로 같은 경고를 얻을 수 있습니다.

`samples/namsan.gpx`는 고도 표현 시연용 합성 경로입니다(실제 기록 아님).

> 타일 제공자의 사용 정책을 반드시 준수하세요. 기본 타일 소스는 없으며, `--vector-tiles` 또는 `--tms-url-template`을 지정해야 지도 배경이 그려집니다. 지도가 없으면 단색 배경입니다.

## 브라우저(WASM) 데모

같은 렌더 코어가 WebAssembly로 컴파일되어 canvas에서 트랙을 애니메이션합니다. 배경 지도는 브라우저가 OpenFreeMap 벡터 타일을 fetch하고, 코어가 CLI와 같은 벡터 렌더러로 그립니다.

2D/3D를 전환할 수 있습니다. 3D는 three.js(WebGL 2)로 그립니다. 지면 타일, 고도 색상 트랙과 커튼, 마커, 라벨 배치는 wasm 코어가 계산한 데이터(`glGround`·`glGroundMesh`·`glTrack`·`glMarker`·`glCamera`·`glLabels`)를 GPU가 그리며, 산 뒤로 넘어간 트랙과 마커는 깊이 버퍼로 판정해 흐리게 비칩니다. `Rotate`나 `Orbit`으로 카메라를 돌리면 매 프레임 정확히 다시 그리므로 끊김이 없습니다(실측 42km 트랙, 지형 포함, 2배 해상도 60fps; 이전 CPU 렌더러는 1배 해상도 약 15fps). 지면은 지도 타일마다 512px 텍스처(밉맵·이방성 필터링)를 지형 위에 덮어 그리며, 카메라에 가까운 곳은 촘촘한 zoom, 먼 곳은 거친 zoom 타일을 매 프레임 고릅니다(텍스처 1텍셀 ≈ 화면 1픽셀). 타일 텍스처는 Web Worker가 그리고, 아직 없는 타일은 이미 그려진 상위 타일이 대신하므로 빈 곳이 생기지 않으며, 오래 쓰지 않은 텍스처는 112장을 넘으면 버립니다. 음영 기복은 정점별로 GPU에서 곱합니다. 3D 지명 라벨은 코어가 WebGL 카메라에 맞춰 매 프레임 배치하고(직전 프레임 라벨 우선, 약 0.4초 페이드, 지형 뒤 라벨 제외 — CLI와 같은 규칙), 같은 폰트로 2D 캔버스에 겹쳐 그립니다(실측 42km, 라벨 켬 약 55fps). WebGL 2를 쓸 수 없는 브라우저에서는 2D만 보여 줍니다. CLI 영상은 계속 Rust CPU 렌더러(tiny-skia)로 그리므로, 브라우저 3D와 영상은 같은 데이터·규칙을 쓰되 픽셀까지 같지는 않습니다. 주소에 `?pmtiles=<URL>`을 붙이면 OpenFreeMap 대신 OpenMapTiles 스키마 PMTiles 아카이브를 HTTP Range로 읽습니다(서버가 Range와 CORS를 허용해야 함). `Labels`와 `Terrain`으로 지명 라벨과 지형(AWS Terrain Tiles)을 켜고 끌 수 있습니다. 카메라와 재생은 지도 위 `View` 패널([lil-gui](https://lil-gui.georgealways.com/))에서 조절합니다: `Map`(지도 스타일: `dark`·`light`, 그리고 3D 건물이 서는 MapLibre `liberty` — 바꾸면 2D 지도와 3D 지면 타일을 새 스타일로 다시 그림, 타일 캐시는 공유; 브라우저는 스프라이트 패턴을 그리지 않음), `Camera` 폴더(3D에서만)의 `Rotate`·`Orbit`·`Tilt`(기울기)·`Follow`(켜면 `Zoom`·`Heading`)·`Reset view`, `Playback` 폴더의 `Speed`(0.25–4배)·`Tail`(마커 뒤로 남기는 트랙 비율, 1이면 전부; 2D·3D가 따로 기억, 기본 2D 0.3·3D 전부로 CLI와 같음; 속도를 바꾸면 추적 카메라의 진행 방향 평균 거리와 회전 속도도 함께 바뀜). 좁은 화면에서는 패널이 접힌 채 시작합니다. 3D 화면을 좌우로 끌면 방위, 위아래로 끌면 기울기(0–75°)가 바뀌고, 휠이나 두 손가락 핀치로 확대합니다(전체 보기에서는 경로 가운데로 최대 4단계, 추적 중에는 추적 zoom). `Reset view`는 방위·기울기·확대를 처음으로 되돌립니다. 기울기와 확대는 카메라 거리만 바꾸므로 지형·라벨 타일을 다시 불러오지 않습니다. `Follow`를 켜면 카메라가 경로 전체 대신 마커를 따라가며, `Zoom`으로 가까이 볼 정도(13–18)를 고릅니다. 넓게 펼쳐진 긴 경로에서 고저와 건물을 가까이 보는 용도로, 회전 슬라이더와 `Orbit`은 그대로 방위를 돌립니다. 마커(고도만큼 올라간 위치)를 화면 가운데, 그 zoom의 거리에 두고, 지형 위에서는 눈 아래 지면보다 그 거리의 0.2배 위에 머물도록 필요한 만큼만 물러섭니다. 추적 중 `Heading`을 켜면 카메라가 마커 뒤에서 진행 방향을 바라보며 따라갑니다(CLI `--follow-heading`처럼, 방향은 재생 약 2초 동안 지나는 거리(최소 150 m)의 평균, 회전은 초당 최대 90°; 드래그·Rotate로 돌린 만큼은 진행 방향에 더해짐). 처음 켤 때 경로를 따라 더 촘촘한 라벨 타일(최대 128장, 긴 경로는 더 거친 zoom)을 불러와 추적 중에는 그 라벨을 씁니다. 추적 중에는 마커가 화면을 일정한 속도(초당 약 240 CSS px)로 지나도록 한 바퀴 시간을 경로 길이에 맞춰 늘립니다. 언덕이 마커를 가릴 때 내려다보는 각을 키우는 CLI의 동작은 아직 없습니다.

```sh
pnpm install
pnpm dev      # build:wasm(wasm-pack) 후 Vite dev 서버 → http://localhost:5173
```

레티나 화면에서는 2D와 3D를 `devicePixelRatio`(최대 2배) 해상도로 그립니다(3D 프로필 패널은 CSS 해상도).

데모는 기본으로 합성 샘플 `samples/namsan.gpx`를 재생합니다. 자신의 기록을 보려면 `samples/local/`에 `.gpx`를 넣으세요(git에서 제외되는 폴더). 이 기록은 개발 서버에서만 쓰이고 `pnpm run build` 결과에는 포함되지 않습니다. 개발 서버에서는 기본으로 `samples/local/`의 첫 기록을 보여 주며, 주소에 `?gpx=<이름>`을 붙여 고를 수 있습니다(`?gpx=namsan`·`?gpx=seoul-b`는 공개 샘플, `?gpx=local/<이름>`은 로컬 기록; 공개 샘플에 없는 이름은 로컬에서 찾고, 없는 이름이면 고를 수 있는 이름을 콘솔에 알리고 기본 트랙을 보여 줌).

`pnpm build`도 wasm 빌드를 선행한 뒤 프로덕션 번들을 만듭니다. 생성된 `crates/wasm/pkg/`는 빌드 산출물입니다.

## 개발

```sh
cargo test --workspace                  # 단위 테스트(core·cli·wasm)
cargo test -p gpx-motion-core --no-default-features   # 브라우저 구성(shaping 없음)
cargo fmt --all                         # 포맷
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p gpx-motion-core --no-default-features --all-targets -- -D warnings
vp run build:wasm && vp check && vp test && vp build   # 웹 데모: wasm 바인딩, 포맷·린트·타입, 테스트, 번들
```

워크스페이스의 기본 멤버는 core·cli뿐이라 `--workspace` 없이 실행하면 wasm 크레이트가 빠집니다.

CI(GitHub Actions)는 push/PR에서 위 검사와 rustdoc 링크 검사, 최소 지원 버전(1.88) 빌드를 실행합니다. `v*` 태그에서는 Linux/macOS/Windows 단일 바이너리와 SHA-256 체크섬을 릴리스합니다. 바이너리의 `--version`은 태그(`v` 제외)를 따릅니다.

## 라이선스 / 출처

- 원본 모델: [gpx-animator/gpx-animator](https://github.com/gpx-animator/gpx-animator) (Apache-2.0)
- 번들 폰트: RobotoMono (SIL Open Font License — `crates/core/assets/font-OFL.txt`)
- 지도 타일: 사용하는 타일 제공자의 라이선스·attribution을 따르세요(OSM 등)
