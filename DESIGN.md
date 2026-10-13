# gpx-motion 설계

GPX 트랙을 지도 위 애니메이션(MP4)이나 이미지(PNG)로 렌더한다. 같은 Rust 코어를 네이티브 CLI와 브라우저(WebAssembly)가 함께 쓴다. 이 문서는 현재 구조를 설명한다. 변경 이력은 [CHANGELOG.md](CHANGELOG.md), 사용법은 [README.md](README.md)에 있다.

## 1. 전체 구조

```
          +------------------- crates/core --------------------+
          | gpx, anim (Timeline), geo (Camera, mercator)       |
          | tiles, vector (style, labels), terrain             |
          | render (tiny-skia)                    -- no I/O    |
          +--------------+----------------------+--------------+
                         |                      |
                         v                      v
               crates/cli (native)      crates/wasm (wasm32)
               reqwest, rayon,          GpxRenderer, SceneRenderer,
               ffmpeg, files            GroundTiles
                         |                      |
                         v                      v
               PNG / MP4                src/ web demo:
               (CPU raster)             three.js (WebGL 2) + Web Workers
```

- **CLI**는 모든 프레임을 CPU에서 `tiny-skia`로 그린다(2D·3D 모두).
- **브라우저 2D**는 같은 CPU 렌더러를 wasm으로 돌린다. **브라우저 3D**는 코어가 계산한 데이터(정점·카메라·라벨 배치)를 three.js가 GPU로 그린다.
- 같은 데이터·규칙(타임라인, 카메라, 지형, 스타일, 라벨 배치)을 쓰지만 브라우저 3D와 CLI 영상이 픽셀까지 같지는 않다.

## 2. 크레이트 경계

### `crates/core` — 플랫폼 중립 코어

GPX 파싱, Web Mercator 투영, 타임라인 보간, 카메라, 벡터 타일(MVT) 디코드와 스타일, 라벨 배치, 지형, `tiny-skia` 래스터를 담는다. 네트워크·파일·인코딩 I/O가 없어서 `wasm32`로도 컴파일된다. 바이트가 필요한 곳(PMTiles 범위 읽기, 타일, DEM)은 코어가 필요한 것을 알려 주고 호출자가 가져와 넘긴다.

| 모듈      | 내용                                                                               |
| --------- | ---------------------------------------------------------------------------------- |
| `gpx`     | `<trkpt>` → `TrackPoint`                                                           |
| `anim`    | `Timeline`(시간·거리 보간), 고도 평활화·누적 상승, 진행 방향 `steer`, `rise_ahead` |
| `geo`     | `mercator`(`Projection`), 원근 `Camera`, 클리핑(`clip`), 지면 타일 LOD             |
| `tiles`   | 타일 주소, 래스터 합성, `pmtiles`(v3 헤더·디렉터리·gzip)                           |
| `vector`  | MVT 디코더, 자체·MapLibre 스타일(`maplibre`), 2D/3D 지도, 건물 돌출, 라벨          |
| `terrain` | DEM → 메시, 소프트웨어 래스터라이저, 깊이 맵, 시선 판정(`hides_point`)             |
| `render`  | 트랙·꼬리·마커, HUD(정보·프로필), 3D 장면, 픽셀 단위 가림, GPU용 장면 데이터       |

**공개 경로 규칙**(`crates/core/src/lib.rs`): 공개 항목마다 경로는 하나다. 모듈 이름으로 한정해 읽는 다섯 모듈(`color`, `math`, `mercator`, `pmtiles`, `maplibre`)만 모듈째 공개하고, 나머지는 크레이트 루트에서 재노출한다(`gpx_motion_core::Timeline`). 정의 모듈은 비공개이며 `unreachable_pub` 경고를 켠다.

**`shaping` 기능**(기본 켬): 아랍어·히브리어·인도계 문자를 `rustybuzz` + `unicode-bidi`로 셰이핑한다. CLI는 켜고 wasm은 끈다. 켜면 wasm이 gzip 기준 568KB에서 768KB로 커져서다(#57 측정). 끄면 그런 이름은 `name:latin`으로 대신한다.

### `crates/cli` — `gpx-motion` 바이너리

| 모듈                       | 역할                                                                                   |
| -------------------------- | -------------------------------------------------------------------------------------- |
| `main`                     | `Args` → `Config` → 트랙·지도·라벨 준비 → 2D 또는 3D 렌더 → `Sink::finish`             |
| `cli`, `color`             | clap 정의와 단일 값 검사(범위, 색, 위치)                                               |
| `config`                   | 옵션 조합 검사. `Config::try_from`이 파일·네트워크를 건드리기 전에 한 번에 검사한다    |
| `frames`                   | `Shot`(`Still`/`Video`), 프레임 공통 입력 `Job`, 출력 `Sink`(PNG/ffmpeg), 병렬 쓰기    |
| `render2d`                 | 평면 뷰: 전체 맞춤(배경 한 번 합성 후 병렬) 또는 마커 추적(프레임 순서대로)            |
| `render3d`, `perspective`  | 3D 뷰: 카메라 경로·지형·라벨을 프레임 순서로 먼저 정하고 프레임은 병렬 렌더            |
| `hud`                      | 고도 프로필, 정보 블록, 출처 표기                                                      |
| `map`, `tiles/`, `sprites` | 지도 소스(래스터 TMS, 벡터 XYZ·TileJSON·PMTiles), 스타일 로드, 스프라이트 패턴 평균 색 |
| `terrain`                  | Terrarium DEM 타일 + 경로 주변 지도 텍스처                                             |
| `labels`                   | 라벨 폰트·색, 라벨이 피할 화면 영역                                                    |
| `track`                    | GPX 파일 → `Track`(타임라인, 팔레트 색)                                                |
| `http`                     | 모든 다운로드가 공유하는 HTTP 클라이언트                                               |
| `encode/ffmpeg`            | rawvideo를 ffmpeg stdin으로 넘겨 H.264 MP4                                             |

- **PNG는 프레임 하나짜리 렌더**다. `Shot::Still`이 영상과 같은 2D/3D 경로를 지나 `Sink::Png`에 쓰인다. 카메라는 영상 첫 프레임처럼 시작점에 두고, 트랙은 끝까지 그린다.
- **ffmpeg는 외부 프로세스**다. `ffmpeg-next` FFI 바인딩은 빌드를 복잡하게 해서 쓰지 않는다. ffmpeg는 출력 옆 숨김 파일(`.<이름>.partial`)에 쓰고, 성공하면 이름을 바꾼다. 실패나 패닉으로 `FfmpegEncoder`가 끝나지 않고 drop되면 ffmpeg를 멈추고 임시 파일을 지운다. 그래서 기존 영상이 남는다.
- **병렬 렌더와 스트리밍 인코딩**: `Sink::write_parallel`이 프레임을 메모리 한도(256MB) 크기의 청크로 나눠 rayon으로 렌더하고, 청크 안에서는 순서대로 쓴다.

### `crates/wasm` — 브라우저 바인딩

| 항목            | 역할                                                                                                         |
| --------------- | ------------------------------------------------------------------------------------------------------------ |
| `GpxRenderer`   | 2D: 보이는 타일 목록(`visibleTiles`), MVT 추가, 지도(`renderMap`)·트랙(`renderOverlay`) RGBA                 |
| `SceneRenderer` | 3D 장면. `CameraRig`(맞춤·궤도·추적 카메라), `LabelState`(라벨 타일·소스·페이드 기억), `TerrainState`로 나뉨 |
| `GroundTiles`   | 지면 타일 하나의 512px 지도 텍스처와 그 위 건물(워커에서 실행, 워커가 실패하면 메인 스레드)                  |
| `PmTiles`       | PMTiles 디렉터리 탐색. 바이트 범위는 JS가 fetch                                                              |
| `layout` 모듈   | JS로 넘기는 평면 배열의 필드 enum과 stride                                                                   |

**JS 이름 규칙**(`crates/wasm/src/lib.rs`):

- `gl*`: 3D 뷰가 매 프레임 받는 것(`glCamera`, `glTrack`, `glLabels`, …)과 그 그리기 상수(`glTailWidth`, `glBackground`)
- `render*`: CPU로 그린 RGBA 이미지
- `set*`/`add*`/`clear*`: 상태 변경
- 그 밖의 이름: 상태 읽기나 가져올 타일 목록(`durationMs`, `labelTiles`, `terrainTiles`)

### `src/` — 웹 데모(Vite+, TypeScript)

모듈마다 폴더가 있고 `*.types.ts`·`*.utils.ts`·`*.test.ts`로 나뉜다. 앱 코드는 상대 경로로 임포트한다. 별칭 `@/`(src)와 `@wasm`(wasm-pack 결과)은 JSDoc 예시에서만 쓰고, `docs:check`가 이 예시를 검사한다.

| 폴더                                                        | 역할                                                                                                                                                     |
| ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `main.ts`, `demo/`                                          | 조립: Effect 서비스와 `Layer`로 데모를 짜고(`startDemo`), 실패하면 대신할 경로를 태그 오류로 정함. 아래 **조립** 참고                                    |
| `app/`                                                      | 조립에 쓰는 부품: 셸 DOM, 트랙 로드, 카메라 조작(`camera-rig`), 2D/3D 전환, 라벨·지형 토글, keepout                                                      |
| `config/`, `controls/`                                      | 데모 설정과 lil-gui `View` 패널                                                                                                                          |
| `playback/`                                                 | 반복 재생 진행률(`playback`), 진행 방향을 따라가는 카메라(`chase-camera`)                                                                                |
| `map-style/`                                                | 스타일 전환(dark·light·liberty). 겹친 전환은 마지막 것만, 실패하면 선택을 되돌림                                                                         |
| `map/flat/`                                                 | 2D 지도: 워커의 `GpxRenderer`가 그림. 워커가 실패하면 메인 스레드에서 같은 그림                                                                          |
| `map/gl/`                                                   | three.js 장면(`gl-scene`), 지면 타일(`ground`, `ground-loader`, `ground-worker`), 트랙 리본·커튼 기하(`geometry`), 셰이더, 라벨 오버레이, 드래그·휠 조작 |
| `map/layout/`                                               | wasm 배열 디코더                                                                                                                                         |
| `map/scene/`                                                | `SceneRenderer` 생성(지형을 쓸 수 없으면 평면으로)                                                                                                       |
| `map/tiles/`, `map/pmtiles/`, `map/terrain/`, `map/labels/` | 타일 소스·캐시, 원격 PMTiles, Terrarium 디코드, 라벨 폰트                                                                                                |
| `worker/`                                                   | 두 워커가 함께 쓰는 타입 있는 메시지 프로토콜                                                                                                            |
| `sample/`                                                   | 재생할 GPX 고르기(개발 서버의 `?gpx=`)                                                                                                                   |
| `viewport/`                                                 | 캔버스를 기기 픽셀 크기로                                                                                                                                |
| `fetch/`, `array/`, `math/`                                 | 응답 검사, 범위 검사 인덱싱(`at`), 각도 계산 같은 작은 유틸                                                                                              |
| `wasm/`                                                     | wasm 초기화와 `withFree`                                                                                                                                 |
| `lifetime/`                                                 | `createLifetime`: `GlScene`·`Ground`·지면 로더·`FlatMap` 안의 정리를 한곳에 모으는 수명(`signal`·`defer`·`end`)                                          |

- **워커 프로토콜**(`worker/`): `createWorkerClient`(메인)와 `serve`(워커)는 메시지 타입마다 함수 하나인 API 타입을 공유한다. 메시지는 보낸 순서대로 하나씩 처리한다. 워커가 시작에 실패하거나, 죽거나, 메시지가 던지면 실패 경로는 하나다. 워커를 멈추고 대기 중인 호출을 reject한 뒤 `onFailure`를 한 번 부른다. 오류는 이름·스택을 담은 데이터로 넘어온다.
- **스타일 전환**은 워커를 새로 만들지 않는다. `FlatView`·`FlatMap`·지면 로더의 `setStyle`이 워커에 `'style'` 메시지(`worker.post('style', …)`)만 보낸다. wasm을 다시 받지 않고, 타일 캐시는 두 지도가 공유한다. 3D는 `GlScene.setMapStyle`로 지면 텍스처와 배경·안개 색을 다시 맞춘다. 2D는 이전 스타일로 늦게 도착한 그림을 버린다.
- **조립**(`demo/`): 데모는 Effect `Layer` 하나다(`demoLayer`). 서비스는 `Stage`(트랙·셸·보기 설정) → `Maps`(2D·3D 지도, 스타일 전환, 타일 채우기) → `Camera` → `Labels`(라벨·지형 토글, keepout) → `ViewControls`(lil-gui 패널, 2D/3D 전환, 드래그)이고, 그 위에서 `mapLoadLayer`(타일 열기)와 `frameLoopLayer`(재생)가 돈다. 서비스마다 구현은 `static layer`로 같은 클래스에 둔다. 프레임 루프는 Effect 밖의 `requestAnimationFrame`이고, 한 프레임은 `draw3d`/`draw2d`로 그린다.
- **실패하면 대신할 경로**는 조립에서 태그 오류로 정한다: `NoWebGl` → 2D만, `MapFailed` → 지도 없이, `TerrainFailed` → 평면 지면, `FontFailed` → 라벨 없이. `TrackFailed`만 데모를 멈추고 `#app`에 오류를 보인다. 워커·타일 하나·스타일 문서의 실패는 각 모듈 안에서 처리한다.
- **정리**: `startDemo()`는 `stop()`을 돌려준다. `stop()`은 데모 파이버를 중단하고, 범위(scope)가 닫히며 등록의 역순으로 정리된다. 각 부분은 최근에 만든 것부터 백그라운드 작업(타일·폰트 로드)을 멈추고, 만든 것(애니메이션, lil-gui 패널, `GlScene.dispose()`의 GPU 리소스·지면 워커, `FlatView.dispose()`의 2D 워커)을 정리한 뒤, 자기 `scopeSignal`로 리스너와 오래 쓰는 요청(타일·스타일 문서)을 함께 끊는다. 한 번만 보내는 요청(트랙·지형·폰트, 타일 소스 열기)은 `Effect.tryPromise`가 넘겨주는 신호를 써서, 그 작업이 중단되면 함께 끊긴다. 타일 소스 열기와 폰트는 `REQUEST_TIMEOUT_MS`가 지나면 끊고 실패했을 때처럼 대신할 경로로 간다. 정리는 모두 동기로 끝나므로, 끊긴 요청의 rejection은 정리가 끝난 뒤에 도착한다. 중단된 요청은 `isAbort`로 가려 경고하지 않는다. 백그라운드 작업에서 예상하지 못한 예외는 `inBackground`가 `console.error`로 남긴다. `GlScene`·`Ground`·지면 로더·`FlatMap`은 안에서 `createLifetime`으로 정리한다. 개발 서버에서는 콘솔의 `stopGpxMotion()`으로 호출할 수 있다.

## 3. 데이터 흐름

### CLI

```
GPX ─parse→ TrackPoint ─→ Timeline(시간·거리·고도) ─┐
                                                   ├→ 2D: Projection(맞춤 또는 추적) ─→ 타일 합성 ─→ 트랙·HUD
옵션 ─Config(검사)─→ Shot, Job ─────────────────────┤
                                                   └→ 3D: Camera 경로(궤도·추적·줌 전환) ─→ 지형·지면 타일·건물
                                                        ─→ 라벨(프레임 순서) ─→ 프레임(병렬)
                                                                    ─→ Sink: PNG 저장 또는 ffmpeg stdin
```

타일은 메모리 캐시와 24시간 디스크 캐시를 거친다. 3D는 벡터 지도에서만 쓸 수 있다. 래스터 타일은 원근으로 변형할 수 없어서다(`tiny-skia`에 원근 변환이 없음). 라벨도 벡터 지도가 필요하다. 래스터 타일에는 이름 데이터가 없어서다. `Config`가 두 조합을 모두 거부한다.

### 브라우저

1. `app/track`이 wasm을 초기화하고 GPX를 받아 `GpxRenderer`(2D)와 `SceneRenderer`(3D)를 만든다.
2. **2D**: 워커가 타일을 받아 `renderMap`으로 지도를 그리고 RGBA를 넘긴다. 메인 스레드는 `putImageData`와 매 프레임 `renderOverlay`(트랙·마커)만 한다.
3. **3D**: 매 프레임 `glCamera`·`glTrack`·`glMarker`·`glGround`(지면 타일 쿼드트리)를 읽어 three.js로 그린다. 지면 텍스처와 건물은 `GroundTiles` 워커가 그린다. 라벨은 `glLabels`가 GL 카메라로 배치하고, 같은 폰트로 2D 캔버스 오버레이에 그린다. 라벨은 코어가 가림·페이드를 정하므로 깊이 테스트가 필요 없고, 한글 SDF 아틀라스도 필요 없다.
4. 가림은 깊이 버퍼로 판정한다. 언덕 뒤 꼬리는 `GreaterDepth` 패스로 흐리게 그리고, 마커는 코어의 `hides_point`로 판정한다.

지면 타일은 프레임마다 고른다(카메라 가까이는 촘촘하게, 텍셀 ≈ 기기 픽셀). 궤도 전체를 덮는 고정 집합은 약 200장(≈200MB)이 필요했다. 아직 없는 타일은 준비된 상위 타일이 대신한다.

## 4. wasm ↔ TS 데이터 계약

- **레이아웃은 Rust 한 곳에 정의한다**(`crates/wasm/src/layout.rs`). 레코드 필드 위치는 `#[wasm_bindgen]` enum(`TrackField`, `CameraField`, `GroundNodeField`, `LabelField`, `GlyphField`, `BuildingField`, `StepField`, …)이고 stride는 함수(`trackStride()` 등)로 내보낸다. Rust 쪽 생산자도 이 필드로 쓴다.
- **TS 디코더**(`src/map/layout/`)는 이 enum과 stride로만 읽는다(`readCamera`, `readTrackPoint`, `forEachGroundNode`, `forEachLabel`, …). 범위를 벗어난 인덱스는 `at()`이 던진다. `layout.test.ts`가 빌드된 wasm과 `namsan` 샘플로 형식을 확인한다.
- **좌표**: GL 위치는 카메라 zoom의 월드 px(x 동쪽, y 남쪽, `h` 위)를 장면 원점에 대해 나타낸 값이다. 원점은 지형의 왼쪽 위 모서리, 지형이 없으면 맞춤 카메라가 처음 겨눈 지점이다.
- **소유권**: wasm-bindgen 객체(`Tile`, `GlLabels`, …)는 JS가 해제한다. `withFree(owned, use)`가 빌려준 뒤 던져도 `free()`한다. `use` 밖으로 객체를 들고 나가면 안 된다.
- **지형은 생성 시점에 정한다**: `new SceneRenderer(…, terrain)`. 지형을 쓸 수 없는 경로(영역이 너무 큼 등)는 메시지가 `no terrain:`으로 시작하는 오류를 던진다. `map/scene`은 이 접두어일 때만 지형 없이 다시 만들고, 그 밖의 오류(GPX 파싱 등)는 그대로 던진다. 접두어는 양쪽 테스트가 확인하는 계약이다.

## 5. 불변 조건

- **GL 원점은 생성 시점에 고정된다.** 지형 여부가 생성자에서 정해지고, 지형이 없을 때의 원점(`start_target`)은 기울기·확대가 바뀌어도 그대로다. 그래서 이미 넘긴 좌표가 계속 유효하다.
- **진행률 `u`**는 `0..=1`로 자른다. NaN(JS에서 올 수 있음)은 `0`(시작)으로 읽고 패닉하지 않는다.
- **GPX 좌표**는 파서에서 검사한다. 유한하지 않거나 범위(위도 `-90..=90`, 경도 `-180..=180`)를 벗어난 `lat`/`lon`은 몇 번째 `<trkpt>`인지 담은 `ParseError::CoordinateOutOfRange`로 거부하고, 유한하지 않은 `<ele>`는 고도 없음으로 읽는다.
- **라벨 페이드 기억**(`LabelMemory`): 같은 우선순위 등급 안에서는 직전 화면의 라벨을 먼저 배치한다(더 중요한 라벨은 여전히 이긴다). 키는 이름 + 대략적 위치(z12 격자)다. 페이드는 `fade_frames` 프레임 동안 진행되고, 첫 화면은 바로 보인다. CLI는 0.4초 × fps, 브라우저는 24프레임이다. 스타일이나 라벨이 바뀌면 브라우저는 소스와 기억을 함께 버린다(`LabelState::invalidate`). CLI는 라벨을 프레임 순서로 먼저 배치하므로 병렬 렌더에도 결과가 같다.
- **클리핑 경계**(`geo::clip::Edges`): 그리기와 충돌 판정은 `Closed`(경계 위 선도 그림)를 쓴다. 타일 사이로 이어 붙이는 선형 라벨 조각은 `HalfOpen`(오른쪽·아래 경계 위 선분은 다음 타일 것)을 써서 중복을 막는다.
- **타일 zoom 전환 페이드**(`mercator::tile_levels`): `--zoom-transition` 영상에서는 타일 zoom이 바뀌는 지점을 가운데 둔 폭 0.3 zoom(±0.15) 구간에서 다음 zoom을 smoothstep으로 겹쳐 그린다. 3D에서 페이드 중인 타일 집합(`Camera::ground_tiles_of_zoom`)은 평소 타일 한도의 4배까지 쓸 수 있다.
- **렌더는 결정적이다.** 같은 입력이면 PNG 바이트와 MP4 프레임이 같다. PNG와 MP4는 같은 렌더 경로(`Shot`)를 쓰므로 한쪽을 고치면 다른 쪽도 바뀐다.
- **3D 카메라는 지형 위에 머문다.** 최소 고도는 지형 최고점 기준(`TERRAIN_CLEARANCE`)이고 CLI와 wasm이 공유한다. 추적 카메라는 눈 아래 지면 기준으로 물러선다.

## 6. 테스트와 CI

- **Rust**: 모듈별 단위 테스트와 doctest(`lib.rs`, `color.rs`, `anim/heading.rs`, `geo/mercator.rs`, `vector/style/json.rs`). `cargo test --workspace`는 wasm 크레이트도 포함한다(기본 멤버는 core·cli뿐). 브라우저 구성은 `-p gpx-motion-core --no-default-features`로 따로 돌린다.
- **벤치마크**: `crates/core/benches/hot_paths`(divan). 공개 API만 쓰므로 crate 내부 함수는 그것이 지배하는 공개 진입점으로 잰다: 삼각형 래스터라이저는 `draw_terrain`, 3D 트랙은 `draw_track_3d`(평지·지형 가림), 레이어 칠하기는 `draw_vector_map`/`draw_vector_map_3d`(dark 프리셋), 3D 라벨은 `LabelSources::collect`와 `place_3d_steady`(한 번에 모은 라벨, 브라우저 경로)·`place_3d_steady_parts`(타일별로 모아 매 프레임 잇는 라벨, CLI 경로). 그밖에 GPX 파싱, `Timeline::build`와 프레임 샘플링. 입력은 공개 `samples/namsan.gpx`, 트랙 일부를 가리는 합성 언덕 DEM(카메라는 CLI처럼 고도를 넣어 맞추고 지형 위로 올린다), 벤치 안에서 인코딩한 합성 MVT 타일(라벨은 번들 라틴 폰트)이라 네트워크가 필요 없다. `cargo bench -p gpx-motion-core`는 빌드를 빼고 10초 남짓에 끝난다. 트랙과 라벨 벤치는 준비 단계에서 실제로 그려지는지(바뀐 픽셀 수, 놓인 라벨) 확인해 컬링만 재는 일을 막는다. CI는 clippy `--all-targets`로 컴파일만 확인하고 실행하지 않는다.
- **TS**: `vp test`(Vitest, node). `layout.test.ts` 등 일부는 빌드된 wasm(`vp run build:wasm`)을 읽는다. `vp run docs:check`(`scripts/docs-check.ts`)는 JSDoc의 ` ```ts ` 예시를 모듈로 꺼내 `tsc`로 타입 검사한다. 예시는 임포트와 입력 선언을 포함해야 한다.
- **린트**: 워크스페이스 `[lints]`에서 clippy pedantic을 켜고, 그래픽 코드의 캐스트 같은 소음 lint는 끈다(이유는 `Cargo.toml` 주석). `missing_docs`도 켠다. TS는 `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`를 쓰고 반환 타입 lint를 켠다.

| CI 작업(`ci.yml`)     | 검사                                                                                             |
| --------------------- | ------------------------------------------------------------------------------------------------ |
| `fmt + clippy + test` | `cargo fmt --check`, clippy(워크스페이스, shaping 없는 core), 테스트 두 구성, rustdoc 링크(`-D`) |
| `MSRV (1.88)`         | `cargo check --workspace --all-targets --locked`(`<[T]>::as_chunks`에 1.88 필요)                 |
| `wasm + web`          | `build:wasm`, `docs:check`, `vp check`(포맷·린트·타입), `vp test`, `vp build`                    |

개발과 CI는 `rust-toolchain.toml`의 툴체인(1.95.0)을 써서 clippy 결과가 같다. `release.yml`은 `v*` 태그에서 Linux·macOS(arm64·x86_64)·Windows 바이너리와 SHA-256 체크섬을 올린다.

**리팩터링 검증**: CLI 쪽 리팩터링(#96–#108)은 변경 전후 CLI 출력을 바이트로 비교했다(PNG, ffmpeg `framemd5`로 본 MP4 프레임). 단 #107은 오류 동작을 일부러 바꿨다. wasm·TS 리팩터링(#109–#112)은 브라우저와 테스트로 확인했다. 자동화된 골든 테스트는 없다. 렌더 경로를 건드리는 리팩터링은 같은 방법으로 확인한다.

## 7. 주요 결정과 이유

- **브라우저·스크린샷 대신 직접 래스터**: GPX Animator의 Java2D 방식처럼 CPU 픽셀 버퍼를 그려 rawvideo로 ffmpeg에 넘긴다. Puppeteer 같은 중간 단계(PNG 직렬화, IPC)가 없다. `tiny-skia`는 순수 Rust라 같은 코드가 wasm으로도 컴파일된다.
- **브라우저 3D는 three.js**: CPU 키프레임 재투영(약 15fps)을 WebGL 2(2배 해상도 60fps)로 바꿨다(#51, #54). CPU 3D 경로는 CLI에만 남았다. WebGL 2가 없으면 2D만 보여 준다.
- **PMTiles**: 코어는 필요한 바이트 범위만 알려 준다. gzip만 지원하고 brotli·zstd는 명확한 오류를 낸다. 순수 Rust 디코더가 wasm을 키우고, 지도용 아카이브는 대부분 gzip이다.
- **MapLibre 스타일**: `background`·`fill`·`line`·`fill-extrusion`을 자체 형식으로 옮긴다. 옮기지 못하는 것(`symbol`, 데이터 의존 값 등)은 종류별로 경고한다. `fill-pattern`·`line-pattern`은 스프라이트 패턴의 평균 색으로 그린다(CLI). 타일은 MapLibre와 같은 데이터 zoom에서 가져온다. 라벨은 MapLibre `symbol` 대신 코어 자체 배치를 쓴다.
- **기본 타일 소스 없음**: 타일 제공자 정책 때문에 CLI는 지도 소스를 사용자가 지정한다. 데모는 OpenFreeMap을 쓴다.

## 8. 참고

- 원본: [gpx-animator/gpx-animator](https://github.com/gpx-animator/gpx-animator) (Java, Apache-2.0)
- [OSM Slippy Map Tilenames](https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames), [PMTiles v3 spec](https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md)
