# gpx-motion — GPX 경로 애니메이션 영상 생성기

> GPX Animator(Java/Swing)의 렌더링 모델을 Rust 코어로 포팅하는 프로젝트.
> 브라우저·screenshot 의존 없이 직접 2D 래스터 렌더링으로 GPX 트랙을 top-down 지도 영상(MP4)으로 변환한다.

---

## 1. 목표

하나 이상의 GPX 파일을 입력받아 이동 경로를 위에서 내려다본 지도 위에 애니메이션으로 그린 영상을 생성한다.

- **입력**: GPX 파일 (1개 이상)
- **출력**: MP4 (H.264)
- **렌더링**: 직접 2D 래스터 (`tiny-skia`), 브라우저/WebGL/screenshot 미사용
- **배포**: 단일 바이너리 (Rust), 추후 napi-rs Node 바인딩 옵션

### 핵심 아키텍처 결정

GPX Animator는 브라우저를 쓰지 않고 Java2D(`BufferedImage` + `Graphics2D`)로 타일을 직접 합성한다.
이 프로젝트도 동일하게 **CPU 메모리상의 픽셀 버퍼를 직접 렌더 → rawvideo로 ffmpeg에 직결**한다.

```
타일 fetch → 2D canvas 합성(타일+트랙+점) → raw RGBA 버퍼 → ffmpeg(rawvideo) → MP4
```

PNG 직렬화나 IPC 왕복 같은 중간 단계가 없어 Puppeteer 방식 대비 수 배~수십 배 빠르다.

---

## 2. 기술 스택

| 영역               | 선택                   | 비고                                                |
| ------------------ | ---------------------- | --------------------------------------------------- |
| 언어               | Rust (edition 2021)    | CPU 바운드 렌더, 단일 바이너리                      |
| 2D 렌더            | `tiny-skia`            | 순수 Rust, Skia 알고리즘. 대안: `raqote`            |
| XML 파싱           | `quick-xml`            | GPX 파싱                                            |
| HTTP               | `reqwest` + `tokio`    | 타일 fetch                                          |
| 이미지 디코드      | `image`                | PNG/JPEG 타일 디코드                                |
| 병렬화             | `rayon`                | 프레임 구간 병렬 렌더                               |
| 인코딩             | ffmpeg (외부 프로세스) | rawvideo stdin 직결. 추후 `ffmpeg-next` 바인딩 검토 |
| CLI                | `clap`                 | 인자 파싱                                           |
| (옵션) Node 바인딩 | `napi-rs`              | 코어를 Node에 노출                                  |

### ffmpeg 의존성

초기에는 시스템 ffmpeg를 외부 프로세스로 spawn한다. 바이너리 자급자족이 필요해지면 `ffmpeg-next` FFI 바인딩으로 전환을 검토한다(빌드 복잡도 증가 트레이드오프 존재).

---

## 3. 모듈 구조

```
src/
├── main.rs           # CLI 진입점 (clap)
├── config.rs         # 렌더 설정 (viewport, fps, total_time, tail 등)
├── gpx/
│   ├── parser.rs     # GPX → Vec<TrackPoint>
│   └── model.rs      # TrackPoint { lat, lon, ele, time }
├── geo/
│   └── mercator.rs   # WGS84 ↔ Web Mercator ↔ pixel 좌표 변환
├── tiles/
│   ├── fetcher.rs    # TMS 타일 fetch + 캐시
│   └── composer.rs   # 뷰포트에 걸친 타일들을 캔버스에 합성
├── render/
│   ├── frame.rs      # 단일 프레임 렌더 (타일 + 트랙 + 점)
│   ├── track.rs      # pre-draw / tail / 현재 위치 렌더
│   └── overlay.rs    # lat/lng·speed·time·attribution 오버레이
└── encode/
    └── ffmpeg.rs     # rawvideo → ffmpeg stdin 파이프
```

---

## 4. 좌표 변환 (렌더의 심장)

WGS84 위경도 → Web Mercator 타일/픽셀 좌표 변환. 모든 렌더의 기준.

```rust
const TILE_SIZE: f64 = 256.0;

pub fn lon_to_pixel(lon: f64, zoom: u32) -> f64 {
    let n = 2f64.powi(zoom as i32);
    (lon + 180.0) / 360.0 * n * TILE_SIZE
}

pub fn lat_to_pixel(lat: f64, zoom: u32) -> f64 {
    let n = 2f64.powi(zoom as i32);
    let lat_rad = lat.to_radians();
    (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI)
        / 2.0 * n * TILE_SIZE
}
```

- zoom 레벨은 전체 트랙의 bounding box가 뷰포트에 맞도록 자동 계산한다.
- 뷰포트가 트랙보다 작으면(`--viewport-width/height` 지정 시) 현재 위치를 따라가는 카메라 이동을 구현한다.

---

## 5. 프레임 루프 + 인코딩

```rust
let total_frames = (total_time_ms / 1000.0 * fps).round() as u64;

let mut ffmpeg = Command::new("ffmpeg")
    .args([
        "-f", "rawvideo", "-pix_fmt", "rgba",
        "-s", &format!("{width}x{height}"),
        "-r", &fps.to_string(), "-i", "pipe:0",
        "-c:v", "libx264", "-pix_fmt", "yuv420p",
        "-crf", "18", "out.mp4",
    ])
    .stdin(Stdio::piped())
    .spawn()?;

let mut stdin = ffmpeg.stdin.take().unwrap();
for frame in 0..total_frames {
    let t = frame_to_timestamp(frame, total_frames, &track);
    let pixmap = render_frame(t, &track, &tiles, &config);
    stdin.write_all(pixmap.data())?;
}
drop(stdin);
ffmpeg.wait()?;
```

- 각 프레임의 타임스탬프에 맞춰 트랙 위치를 **선형 보간**한다.
- 병렬화 시: 프레임 구간을 rayon으로 나눠 `Vec<Pixmap>` 생성 후 순서대로 write (메모리 vs 속도 트레이드오프 주의 — 스트리밍 인코딩과 병렬 렌더의 조율 필요).

---

## 6. CLI 인터페이스

GPX Animator 옵션을 참고해 1:1에 가깝게 설계한다.

```
gpx-motion \
  --input track.gpx \           # 필수, 복수 지정 가능
  --output movie.mp4 \          # 기본 movie.mp4
  --viewport-width 640 \
  --viewport-height 640 \
  --total-time 120000 \         # ms, 미지정 시 트랙 실제 시간 기반
  --fps 30 \
  --tail-duration 10000 \       # ms, 꼬리 길이
  --pre-draw-track \            # 전체 경로 미리 회색으로
  --pre-draw-track-color '#808080' \
  --tms-url-template 'https://tile.openstreetmap.org/{zoom}/{x}/{y}.png' \
  --background-map-visibility 1.0 \
  --information-position 'bottom left' \
  --attribution-position hidden
```

---

## 7. 로드맵 (수직 슬라이스)

전체를 한 번에 옮기지 않고, 각 단계가 "동작하는 결과물"을 내도록 수직으로 쌓는다.

### Milestone 1 — 트랙 PNG 한 장

- [x] GPX 파싱 (`gpx/parser.rs`)
- [x] 좌표 변환 (`geo/mercator.rs`)
- [x] 단색 배경 + 트랙 폴리라인을 PNG 한 장으로 출력 (지도 타일 없이)
- ✅ 산출물: `track.png`

### Milestone 2 — 점 이동 영상

- [x] 시간 보간 + 프레임 루프
- [x] ffmpeg rawvideo 파이프 (`encode/ffmpeg.rs`)
- [x] tail(꼬리) 렌더
- ✅ 산출물: 배경 없는 점 이동 MP4

### Milestone 3 — 지도 배경

- [x] TMS 타일 fetch + 캐시 (`tiles/fetcher.rs`)
- [x] 뷰포트 타일 합성 (`tiles/composer.rs`)
- [x] zoom 자동 계산
- ✅ 산출물: 지도 위 트랙 애니메이션 MP4

### Milestone 4 — 디테일

- [x] pre-draw-track (전체 경로 사전 렌더)
- [x] 정보 오버레이 (lat/lng, speed, time)
- [x] attribution 표시/숨김
- [x] 카메라 이동 (뷰포트 < 트랙)
- [x] 복수 GPX 입력

### Milestone 5 — 성능 & 배포

- [x] rayon 프레임 병렬화
- [x] 타일 캐시 디스크 영속화 (24h TTL)
- [ ] (옵션) napi-rs Node 바인딩
- [x] CI 빌드 (멀티플랫폼 단일 바이너리)

### Milestone 6 — 벡터 지도

래스터 타일은 정수 zoom 고정, 고해상도에서 흐림, 스타일·라벨 제어 불가라는 근본 한계가 있다. MVT(OpenMapTiles 스키마)를 코어에서 `tiny-skia`로 직접 그려 CLI와 브라우저가 같은 지도를 렌더한다. 라벨은 텍스트 배치 비용이 커서 뒤로 미루고, 트랙 영상에 방해가 되지 않는 라벨 없는 지도부터 만든다.

- [x] 의존성 없는 MVT 디코더 (`vector/mvt.rs`, wasm 호환)
- [x] 라벨 없는 스타일 프리셋 `light`/`dark` (`vector/style.rs`)
- [x] 벡터 렌더 + overzoom (소스 maxzoom 초과 시 타일 확대, 선명도 유지)
- [x] CLI `--vector-tiles` (TileJSON 또는 XYZ), `--map-style`
- [x] 브라우저 데모를 같은 벡터 렌더러로 전환
- [x] zoom에 따른 도로 폭 보정(z14 기준 줌당 √2배, 0.35–3배), 화면 배율(`--pixel-ratio`, 브라우저 2D `devicePixelRatio`) (#5)
- [x] 실수 zoom: fit이 경로를 여백 없이 채움, `--zoom` 소수 허용, 래스터 타일 확대 합성 (#6)
- [x] 영상 중 카메라 zoom 애니메이션: `--zoom-transition`으로 시작·끝에 전체 경로 ↔ 추적 zoom 이징. 2D는 zoom을 선형, 중심을 두 화면이 공유하는 고정점 기준으로 보간(순수 줌), 3D는 추적 zoom의 월드에서 거리·조준점을 같은 방식으로 보간해 지형과 함께 동작 (#23). 중간 zoom 프레임은 2D 추적처럼 순차 렌더, 3D는 `Camera::detail_zoom`(양 끝 zoom 사이 선형)으로 도로 폭·타일 zoom을 정해 줌 아웃 화면이 일반 3D와 같은 세부를 가짐. 줌 전환 품질(#44): 줌 전환 영상에서만, 타일 zoom이 바뀌는 곳에서 `tile_levels`가 0.3 zoom 폭에 걸쳐 다음 zoom 타일을 smoothstep으로 겹쳐 그림(두 레이어를 섞은 뒤 지도 투명도로 합성, 2D·3D). 3D는 타일 수 한도(64장)가 거리에 따라 zoom을 낮추므로, 상세 zoom과 발자국 면적으로 구한 연속 한도 zoom(`Camera::ground_budget_zoom`) 중 낮은 쪽을 기준으로 페이드(겹치는 동안은 한도 4배). 지형 텍스처는 박스 필터 밉맵 + 삼각형마다 축소율로 두 레벨을 섞는 트라이리니어 샘플링. 지형은 격자선이 고르지 않은 한 메시(주 영역은 촘촘히, 둘레는 성기게, 경계선을 공유해 틈 없음)에 텍스처 여러 장(drape)을 얹을 수 있어, 줌 전환 시 전체 경로 zoom의 성긴 DEM·지도로 경로 크기만큼 둘레를 더함(`Terrain::with_surroundings`). 높이 감쇠는 바깥 경계에만. 카메라 최소 고도는 주 영역 최고점(`core_max_height`)과 눈 아래 지면 높이로 정해, 먼 둘레 산 때문에 추적 카메라가 높아지지 않음(주 영역 경계의 감쇠가 사라져 경계 근처 봉우리가 있으면 줌 전환 없는 영상과 고도·내려다보기가 조금 다를 수 있음). 비용: 실측 합성 30km 경로 3D·지형·3초 전환 ×2(10초 영상)에서 9.7초 → 13.7초(넓은 텍스처 1회 그리기, 경계 프레임의 이중 지도 그리기)
- [x] 오프라인 PMTiles 소스: 코어에 v3 헤더·디렉터리·Hilbert tile id·gzip 해제, CLI 로컬 `.pmtiles`, HTTP MVT gzip 자동 해제 (#7)
- [x] 원격·브라우저 PMTiles: core `Archive`가 I/O 없이 필요한 바이트 범위(헤더+루트 16KiB, 리프 디렉터리, 타일)를 알려 주고 호출자가 가져옴. CLI는 파일 또는 HTTP Range(206 필수, ETag 변경 감지, Range 무시 서버는 명확한 오류), 브라우저는 wasm `PmTiles` + fetch(`?pmtiles=`). 디렉터리는 실행 동안 메모리에 캐시. brotli·zstd는 지원하지 않음(순수 Rust 디코더가 wasm 크기를 키우고, 지도용 아카이브는 대부분 gzip) — 해당 아카이브는 명확한 오류 (#24)
- [x] 사용자 스타일 파일: 자체 JSON(`extends`, `background`, `rules`), CLI 경로·브라우저 문자열, 예시 `styles/` (#8)
- [x] MapLibre style JSON 가져오기: `background`/`fill`/`line` 레이어, 레거시·표현식 필터(`vector/expr.rs`), zoom 곡선 선 폭(MapLibre zoom = 256px 월드 zoom − 1을 CSS로 환산), 레이어 zoom 범위, 점선, 색·투명도(zoom 13 값). 자체 형식도 `filter`/곡선 `width`/`dash`/`minzoom`·`maxzoom`을 받아 가져온 결과를 손실 없이 내보냄(`--export-style`). zoom 곡선 투명도, 선 레이어의 면 외곽선, `line-cap`, MapLibre식 홀수 점선, `fill-extrusion`은 평면 채우기, 반투명 면은 타일 경계 이중 블렌딩을 막도록 타일 사각형으로 자름, CLI는 MapLibre 데이터 zoom(`floor(MapLibre zoom)`)에서 타일을 가져옴. 그 밖의 것은 종류별로 묶어 경고 (#25). 미지원: `line-gap-width`·`line-offset`·`line-join`, 패턴, 데이터 의존 값, `symbol`, zoom에 따라 바뀌는 색(한 값으로 고정), 브라우저의 데이터 zoom
- [x] MapLibre 가져오기 확장: 공개 스타일 5개(OpenFreeMap liberty·bright·positron·dark·fiord)에서 무시되던 것을 빈도순으로 — `line-join`(스타일마다 21–47개 레이어, 기본 miter·miter limit 2), zoom에 따라 바뀌는 색(5개 모두; `Rule::colors` 색 곡선, 레거시 stops·`interpolate`, RGB 보간), `fill-outline-color`(1px 외곽선 규칙 추가), `fill-antialias: true`(원래 그렇게 그리므로 경고 안 함). 자체 형식에 `join`·`colors` 추가(내보내기 왕복). 남은 경고: `fill-antialias: false`, `line-blur`, `line-gap-width`, `fill-translate`, 패턴, 3D 높이 (#43 일부)
- [x] MapLibre 가져오기 확장 2(#43): `fill-antialias`(`false`·zoom `step`, 스타일 4개 8곳; `Rule::antialias` 0/1 곡선, tiny-skia 안티앨리어싱 끔), `line-blur`(dark·fiord 경계 6곳; MapLibre는 선 폭 안쪽 `blur` px에 걸쳐 알파를 낮추므로 그 평균만큼 옅게 그리는 근사), `fill-translate`(bright 건물 지붕; `OffsetCurve`, 지도 축 기준 CSS px — 3D에서는 상세 zoom의 px를 월드 px로 환산, `viewport` 앵커는 경고), `line-gap-width`(liberty 터널; 경로 경계 크기 임시 픽스맵에 넓은 선을 그리고 가운데를 지워 합성), `step` 색. 브라우저 2D `visibleTiles(maxZoom, style)`가 MapLibre 스타일이면 데이터 zoom 사용(3D 지면은 512px 텍스처라 이미 같음). 공개 스타일 5개에서 남은 경고는 패턴 채우기 3개 레이어와 3D 건물 높이
- [x] 스프라이트 패턴(#43): CLI가 스타일의 `sprite`(URL 하나 또는 `{id, url}` 목록, `id:이름`)에서 JSON·PNG를 받아 쓰이는 패턴마다 알파 가중 평균 색(투명 픽셀은 색이 아니라 투명도로)을 구하고, `from_maplibre_with`가 `fill-pattern`·`line-pattern` 레이어를 그 색으로 그림(멀리서 보면 반복 무늬가 그 색에 가까움). 스프라이트에 없는 패턴은 MapLibre처럼 그리지 않음(dark `wood-pattern`). 공개 스타일 5개에서 옮기지 못하는 것은 3D 건물 높이와, 스프라이트에 없는 dark `wood-pattern`(MapLibre도 그리지 못함)뿐
- [x] 라벨: 사용자/시스템 한글 폰트, place·peak·water_name 점 라벨, 중요도순 탐욕 배치(라벨·트랙·오버레이 회피), halo, 3D 깊이 축소 (#9, CLI)
- [x] 브라우저 라벨: JS가 폰트 바이트(NanumGothic)를 전달, 2D 지도와 3D 장면 모두. 3D는 라벨 레이어만 디코드(`VectorTile::decode_layers`)해 키프레임 워커와 독립, 키프레임별로 라벨 후보를 캐시(`LabelSources`)하고 방위마다 화면에서 배치해 지면 캐시와 함께 보관. 라벨 배치·3D 커튼 외곽선·대비 색상을 core로 옮겨 CLI와 공유, 후광은 글리프 외곽선을 한 번만 읽음 (#27). 회전 중 실측 42km에서 ~30 → ~25fps
- [x] 3D 라벨 안정화: `LabelMemory`가 직전 화면의 라벨을 이름+대략적 위치로 기억해 같은 우선순위 등급 안에서 먼저 배치(히스테리시스, 더 중요한 라벨은 여전히 이김), 0.4초 페이드 인·아웃(첫 프레임은 바로, 지형에 가려지는 라벨도 페이드 아웃). CLI는 프레임 순서로 라벨만 먼저 배치한 뒤 병렬 렌더, 브라우저는 방위가 바뀔 때마다 이어서 배치(페이드 없음). 지형 뒤 라벨은 카메라까지의 시선을 지형 높이와 비교해 제외(`Terrain::hides_point`). 셰이핑이 필요한 문자는 `name:latin`으로 대체 (#28)
- [x] 선형 라벨: `transportation_name`·`waterway`·선형 `water_name`의 이름을 화면에 투영한 선을 따라 글자 단위로 배치(이웃 글자 사이 꺾임 30° 이하, 왼쪽→오른쪽으로 읽히게 뒤집기, 280px 간격 반복, 도로 등급 순). 글리프 아웃라인을 tiny-skia 경로로 바꿔 회전해 그리고 후광은 경로 스트로크. 점 라벨 다음 순서로 같은 충돌 검사(글리프별 상자)·keepout·라벨 기억(3D 영상)·지형 가림(라벨의 양 끝·가운데 글자 아래 지면만 시선 검사)을 공유, 3D에서는 선의 월드 중간점에 맞춰 놓아 화면에서 미끄러지지 않게 하고 페이드 인·아웃. 도로 등급별 최소 zoom(작은 도로는 z15부터), CLI는 타일별 라벨 소스를 프레임 사이에 캐시 (#26). 브라우저 회전 중 비용(실측 42km, 라벨+지형 끔 20.8 / 라벨 켬 15.3fps), CLI 3D 궤도 240프레임 5.9 → 6.65s
- [x] 라벨 셰이핑: `TextFont`가 셰이핑이 필요한 문자(`needs_shaping`)만 rustybuzz로 셰이핑 — 유니코드 양방향 알고리즘(`unicode-bidi`)으로 방향별 구간을 나눠 각각 셰이핑하므로 오른쪽→왼쪽 이름 안의 숫자·라틴 단어도 제 순서(예: "شارع 26 يوليو"), 결과는 이름별로 폰트 단위로 캐시(크기 무관, 셰이핑 비용이 글자 단위의 40–70배라), 나머지는 글자 단위 그대로(배치 비용 유지). 점 라벨·선형 라벨(`draw_turned`이 셰이핑 글리프 단위) 모두, 셰이핑 문자의 선형 라벨은 총 꺾임 15° 이하(이어진 글자는 꺾이지 않음, 브라우저 오버레이는 통째로 그림). 이름은 폰트가 그릴 수 없을 때만 `name:latin`. core `shaping` 기능(기본 켬, CLI 켬, wasm 끔: wasm 1.22 → 1.75MB, gzip 568 → 768KB라 브라우저는 라틴 대체 유지). 테스트용 Noto Sans Arabic 부분 집합 8KB(OFL) (#40)
- [x] 선형 라벨 품질: 타일 조각을 타일 경계로 자르고(버퍼 중복 제거, 오른쪽·아래 경계 위 선분은 다음 타일 것) 같은 이름·종류·타일 zoom 조각을 끝점끼리 이어 붙여(허용오차 4/4096 타일, 양방향) 한 도로가 한 줄기 — 조각마다 짧아 빠지던 이름이 경계를 넘어 붙음. 사본 위치는 각 타일 조각의 가운데(로드된 다른 타일과 무관하게 고정) ± 반 간격의 배수(조각 안, 그다음 조각 끝)에 두어 시점·로드된 타일이 바뀌어도 도로 위 제자리에 머묾 — 이어 붙인 줄기의 가운데를 쓰면 CLI처럼 프레임마다 타일 집합이 바뀔 때 사본이 튀고 기억 키가 바뀜. 기억 키도 조각 가운데별로 두고 그중 최댓값을 이어받음. 사라진 선형 라벨도 페이드 단계를 기억해 잠깐 뒤 다시 나타나면 이어서 진행. 8초 3D 영상에서 이름 단위 깜박임 seoul 27 → 25(사본 30.3 → 32.4/프레임), seoul-b 23 → 17, 실측+지형 16 → 7(점 라벨·CPU 시간 변화 없음). 시도했다 되돌린 것: 사라지는 선형 라벨도 다른 라벨과 겹치지 않게 하면 갑자기 사라져 깜박임이 늘어남 (#41 일부)
- [x] 라벨 가장자리·출처 표기: 화면을 벗어나는 라벨은 바로 사라지지 않고 페이드 아웃 동안 화면 밖으로 걸쳐 그려짐(점 라벨 앵커는 화면 밖 48px까지 후보로 두되 새로 놓지는 않음, 선형 라벨 사본은 화면에 걸친 글자만 맞으면 됨). 렌더러에 라벨 keepout 사각형을 넘기는 `setLabelKeepout`(2D `GpxRenderer`, 3D `SceneRenderer`), 데모는 출처 표기의 위치를 재서(창 크기 변경·지형 출처 추가 시 다시) 넘김. 8초 3D 영상 이름 단위 깜박임: 선형 seoul 25 → 19, seoul-b 17 → 15, 실측 7 → 8, 점 라벨은 비슷(3 → 5, 28 → 29, 24 → 21); 가장자리에서 페이드 중인 라벨이 자리를 차지해 seoul-b의 선형 사본은 프레임당 1.8 → 1.1 (#42)
- [x] 남은 라벨 깜박임(#41): 원인을 프레임별로 계측하니 보이던 라벨이 페이드 없이 사라지는 경우가 셋 — 페이드 중인 사본이 keepout(정보 표시·트랙)에 걸림, 먼 사본이 최소 크기 경계를 오감, 점 라벨이 상한(60개)을 채워 그 뒤에 놓이는 선형 라벨이 밀려남. 페이드 중인 선형 라벨은 keepout에 걸린 글자만 빼고 그림(`PlacedLabel::covered`; 덮개·트랙 아래로 들어가는 모양, 셰이핑 문자는 통째로만), 페이드 중인 라벨은 최소 크기의 0.75배까지 유지, 상한은 직전 프레임에 보이던 라벨(선형은 그때의 사본 수)에 먼저 자리를 잡아 두고 새 라벨이 나머지를 채움(화면 바로 밖에서 페이드 중인 라벨도 예약; 전체는 60개 근처, 페이드 중인 것과 끊긴 선이 갈라질 때만 잠시 넘음; 실측 최대 60개). 눈에 보이는 선형 라벨 깜박임(불투명도 0.3 이상): seoul 11→4, seoul-b 12→0, 실측+지형 6→3(남은 것은 사본이 정보 표시·출처 표기 영역에 통째로 들어간 경우), 점 라벨 깜박임 seoul-b 29→18
- [x] 사라지는 선형 라벨의 겹침과 회전 중 배치 비용(#41): 페이드 중인 사본은 keepout뿐 아니라 다른 라벨과 겹치거나 언덕 뒤에 놓이는 글자도 빼고 그려 무엇과도 겹치지 않음(예전에 되돌린 시도는 겹치면 라벨 전체를 뺐다면, 이번에는 글자 단위라 대부분 일부만 빠짐)(사본이 통째로 가려지면 사라짐; 눈에 보이는 깜박임 seoul 4→5, 실측+지형 3→5). 배치 비용은 프로파일(이름을 남긴 wasm)로 찾아 줄임: 지형 최고점을 매 시선 판정마다 다시 구하던 것을 미리 계산, 카메라 회전의 sin/cos을 점마다 계산하던 것을 `Camera::projector`로 한 번만, 라벨 기억 맵을 FxHash로, 균일한 지형 격자는 이분 탐색 대신 나눗셈으로 칸을 찾고, 선형 라벨 글자 폭은 크기 1로 한 번 재 두고 비례. 브라우저 라벨 켠 궤도 회전(남산, 프로덕션 빌드) 43–45 → 53–57fps, 20ms 넘는 프레임 5초에 76–83 → 17–36회. 남은 비용은 대부분 매 프레임 CPU로 그리는 고도 프로필 패널(프레임당 약 3.6ms)

### Milestone 7 — 고도 표현

핵심 가치는 지도보다 GPX 고도에 근거한 표현이다. 2D 표현을 먼저 쌓고, 3D는 벡터 지도의 꼭짓점을 원근 투영하는 방식으로 붙인다(래스터는 `tiny-skia`가 원근 변환을 지원하지 않아 제외).

- [x] 타임라인 고도 보간, 누적 거리, 누적 상승/하강(3m 히스테리시스) (`anim/elevation.rs`)
- [x] 고도 평활화: 거리 기준 100m 이동평균(`--elevation-smoothing`), 색상·프로필·수치·3D 높이에 공통 적용. 경주 실측 42km에서 누적 상승 +364m → +209m
- [x] 정보 오버레이에 고도·누적 상승/하강
- [x] 고도별 트랙 색상 램프 (`render/ramp.rs`)
- [x] 하단 거리-고도 프로필 그래프, 진행 구간 강조 + 마커 (`render/profile.rs`)
- [x] 원근 카메라: pitch/bearing/fov, near-plane 클리핑, 투영 중심 보정 fit, 궤도 회전 (`geo/camera.rs`)
- [x] 3D 트랙: 고도만큼 상승 + 지면 커튼(깊이 정렬), 마커 그림자/낙하선, 원경 안개 (`render/scene.rs`)
- [x] 브라우저 데모 3D: `SceneRenderer`(wasm), 고정 카메라 + 회전 슬라이더, 궤도 전체 지면 타일 선로드
- [x] 지도 렌더 최적화: 화면 밖·1.5px 미만 도형 생략, 0.75px 미만 꼭짓점 병합, 타일 단위 경로 분할
- [x] 궤도 회전 시 방위마다 조준점 재계산(거리는 전 방위 최댓값 고정)
- [x] 긴 실측 기록 대응: 3D 트랙 1.5px 샘플 솎기 + 색상 32단계 양자화로 같은 색 구간 일괄 그리기, 프로필 0.5px 솎기
- [x] 브라우저 연속 궤도 회전: 지면을 10° 키프레임으로만 정확히 렌더하고, 사이 방위는 두 키프레임을 호모그래피로 재투영 (#3, #50에서 WebGL로 대체해 제거). 회전 중 ~9fps → ~30fps(실측 42km 트랙, 키프레임 캐시 후). 키프레임 첫 방문 멈춤(~220ms)은 Web Worker 렌더로 제거: 현재 방위에 가까운 키프레임부터 워커가 그려 전달하고, 도착 전에는 가장 가까운 캐시 키프레임으로 재투영 (#20). 15초 자동 회전에서 100ms 넘는 프레임 17회 → 0회
- [x] 원근에 따른 도로 폭 보정(깊이별 1/8 단계 배율), 3D 꼬리 페이드(`--tail-duration`) (#2)
- [x] 마커 진행 방향을 따르는 추적 카메라: 거리 창 chord 방위 + 프레임당 회전 속도 제한 (#1)
- [x] 지형(DEM) 기반 지면 기복: Terrarium 타일, 경로 주변 지도 텍스처 + hillshade를 삼각형 메시에 매핑(화가 알고리즘), 가장자리 높이 감쇠, 트랙·커튼·그림자를 지형 위에 (#4, CLI)
- [x] 브라우저 지형(#50에서 WebGL 지면으로 대체해 제거): Terrarium 타일을 JS가 디코드해 전달, 지도 텍스처는 키프레임 워커가 그림(`renderTerrainTexture`), 키프레임은 평면 지도 그대로 재투영하고 그 위에 지형 메시를 방위마다 정확히 그려 지면 캐시에 함께 보관 (#21). 지형 삼각형을 직접 스캔라인으로 채움: 꼭짓점을 1/256px로 맞춰 정수 에지 함수로 판정(공유 에지·꼭짓점의 픽셀이 정확히 한 삼각형에 속함), 텍스처는 고정소수점 bilinear. tiny-skia 패턴 대비 640px 10.6 → 6.3ms, CLI 출력 PSNR 69.5dB. 텍스처 최대 2048px(메인 16MB, 워커에 그릴 때 16MB 추가). 회전 중 실측 42km에서 라벨+지형 ~20fps
- [x] 지형에 의한 트랙 가림: 지형 래스터화 때 픽셀별 1/깊이(화면에서 선형이라 정확히 보간)를 남기고, 트랙 샘플이 3×3 이웃 모두보다 1% 넘게 뒤에 있으면 숨김(한쪽 끝이라도 숨은 구간은 커튼 없이 흐린 가는 선으로, 숨은 마커는 흐리게). 실측 궤도 영상 기준 오검출 0, 놓친 가림 18.5%(5%일 때) → 4.9%. 지형 위에서는 트랙을 월드 8px 간격으로 세분화해 각 점을 지면 위로 올림 (#22). 단일 픽셀 비교는 비스듬한 시점에서 지면 위 트랙을 42% 잘못 가려 3×3 이웃이 필요
- [x] 브라우저 3D를 three.js(WebGL)로: 코어는 렌더러 중립 데이터만 내보내고(`track_vertices`·`marker_vertex`·`fog_band`, `Terrain` 격자·텍스처, `Camera::target`·`fov_deg`), wasm `glTerrain`·`glTrack`·`glMarker`·`glCamera`가 지형 원점 기준 월드 px로 전달. 지형은 코어와 같은 대각선으로 나눈 메시 + 밉맵 텍스처(가장자리에서 배경색으로 페이드), 트랙은 화면 폭 리본 셰이더, 커튼·꼬리 페이드는 진행률 유니폼으로 GPU에서 판정. 가림은 깊이 버퍼: 모든 패스가 깊이 비교 때만 1% 눈 쪽으로 당겨지고(화면 위치 불변), 숨은 꼬리는 `GreaterDepth` 패스로 흐리게, 마커는 CPU와 같이 코어의 시선 판정(`hides_point`)으로 통째로 흐리게. 반투명 패스는 패스마다 스텐실 값을 달리해 겹친 이음매를 한 번만 섞고, 선분은 반 폭만큼 늘려 꺾인 곳의 틈을 메움. WebGL 2가 없으면 CPU 렌더러로 대체(#50에서 2D만 보이도록 바꿈). 카메라는 지형 최고점의 1.2배 위(`TERRAIN_CLEARANCE`, CLI와 공유). 실측 42km·지형 켬에서 2배 해상도 60fps(CPU 렌더러 1배 약 15fps). 지면은 처음엔 지형 영역 텍스처 한 장(#49에서 타일로), CPU 경로는 `?renderer=cpu`로 유지(#50에서 제거) (#47)
- [x] 브라우저 CPU 3D 경로 제거: 키프레임 재투영(`reproject_ground`, `Camera::overscanned`·`ground_to_screen`·`ground_tiles_at`), 키프레임 워커, 지면 캐시, CPU 라벨·지형 텍스처 API를 지우고 `SceneRenderer`는 WebGL 데이터만 내보냄. 라벨 타일은 `labelTiles` 하나로(지형 영역, 없으면 궤도 36방위의 지면 타일). WebGL 2가 없으면 3D 버튼을 끄고 2D만. 브라우저 = three.js 렌더러, CLI = CPU 렌더러(tiny-skia), 공통 = Rust core(GPX·타임라인·카메라·지형·타일·스타일·라벨 배치) (#50)
- [x] WebGL 지면 타일: 코어 `ground_tiles`가 현재 카메라로 지면 타일 쿼드트리를 고름 — 화면에 보이는 타일을 텍셀이 기기 픽셀보다 거친 정도(눈에서 가장 가까운 지점 기준)가 큰 것부터 나눔(최대 128개, z18까지, 카메라 뒤·화면 밖은 나누지 않음). 궤도 전체를 위한 고정 집합은 눈이 원 안 어디든 가까이 올 수 있어 약 200장(≈200MB)이 필요해 프레임별 선택으로 바꿈. 타일 텍스처는 512px(2× 선폭, 데이터 zoom 넘으면 overzoom)로 `GroundTiles`(wasm)가 워커에서 그림, 지형은 텍스처 없이 높이만(`Terrain::bare`) 만들고 음영은 정점 속성(`light_at_world`). 메시는 타일 안의 지형 격자선과 그 중간선을 모두 정점으로 넣어(각 셀이 지형 삼각형 하나 안에 있거나 지형 대각선을 따라 나뉨) 트랙이 서는 지형과 정확히 일치(타일 가장자리 잘린 셀만 1px 미만 차이), 가장자리에 스커트를 내려 zoom이 다른 이웃 사이 틈을 가림. 그리기 집합은 준비된 타일로 리프를 덮되 못 덮는 곳은 준비된 상위 타일로 대신(겹침 없음), 요청은 리프와 아직 자식으로 못 덮는 상위 타일만 거친 zoom부터 2개씩, 업로드는 프레임당 1장, 텍스처 112장 LRU(그려지는 타일은 제외). 지형 영역이 없어도(긴 경로) WebGL로 그림. 지면 갱신·그리기 제출은 프레임당 약 1ms; 라벨 켠 회전은 54–56fps로, 남은 비용은 2D 라벨 오버레이의 텍스트 래스터(계산만 하면 60fps) (#49)
- [x] WebGL 3D 라벨: wasm `glLabels`가 GL 카메라로 `place_3d_steady`를 매 프레임 실행(24프레임 페이드 `LabelMemory`, 라벨 소스는 지형 지도 타일 전체에서 한 번 수집)하고 기준선·CSS 글꼴 크기(`TextFont::em_size`, ab_glyph 크기는 em이 아니라 ascent−descent)·후광 폭·글자별 위치/각도를 넘김. 그리기는 같은 폰트(`FontFace`)로 2D 캔버스 오버레이: 배치·가림·페이드를 코어가 라벨 단위로 정하므로 픽셀 깊이 테스트가 필요 없고, 한글 SDF 아틀라스 없이 레티나에서 선명함. 지형 시선 검사(`hides_point`)는 들어갈 자리가 있는 라벨에만 하도록 바꿔(결과 동일) 라벨 켠 회전 47 → 58fps(라벨 끔 60fps, 배치 중 지형 검사가 약 6ms였음) (#48)
- [x] 추적 카메라의 마커 시야: `--zoom` 카메라가 지형 위에서 프레임마다 마커까지의 시선(`hides_point`)이 열릴 때까지 pitch를 2.5°씩 낮추고(최소 10°), `rise_ahead`(창 1초: ±2초 최댓값의 ±1초 이동 평균 — 3초 전부터 기울기 시작해 1초 전에 다 기울고, 지난 뒤 1초 유지 후 2초에 걸쳐 복귀, 각 프레임 필요량 이상 보장)로 완만하게 적용. 8초 영상에서 렌더 깊이 기준으로 마커가 가려진 프레임 namsan z15 p75 48 → 0, seoul 36 → 3, 실측 42km z14 p75 5 → 0. 남은 2–3프레임은 지면에 붙은 마커에서 시선 판정과 깊이 맵(3×3 허용오차)이 엇갈리는 경우 (#38)
- [x] 픽셀 단위 가림(CLI): 커튼·고스트선·꼬리선의 1/깊이를 종류별로 따로 래스터(`NearDepth`, 화면에서 선형이라 정확, 안티앨리어싱 가장자리용 1px 확장, 덮인 범위만 처리)하고 지형 깊이 맵과 픽셀마다 비교(3×3, 1% 허용오차 그대로)한 마스크로 각각 그림 — 가까운 부분이 먼 부분을 비쳐 보이게 하지 않고(깊이 하나로 레이어를 고르면 가까운 커튼 위로 언덕 뒤 선이 밝게 나옴), 언덕 뒤 커튼·고스트는 사라지며 꼬리는 흐린 가는 선만. 가려진 픽셀이 없는 부분은 마스크 없이 그림. 경계가 8px 구간이 아닌 픽셀 단위. 720p 300프레임 CPU 시간 12.9 → 14.7s(+13%) (#39)

- [x] 3D 건물(CLI): 스타일 규칙의 `extrude`(MapLibre `fill-extrusion-height`·`base`·`vertical-gradient`)로 지면 타일의 면을 각기둥으로 세움(`vector/extrusion.rs`). 외곽·구멍 고리를 첫 고리의 감김 방향 기준으로 묶고(MapLibre `classifyRings`처럼) 타일 사각형으로 자르며, 잘린 변에도 벽을 세워 지형 때문에 조각마다 바닥이 다를 때 생기는 단차를 막음(높이가 같으면 안쪽이라 가려짐; 오목한 건물을 자르면 생기는 폭 0 다리 변은 원래 고리가 경계를 지나는 곳에서 나눠 건물 쪽 구간만 벽으로), 지붕은 earcut으로 삼각분할, 바닥은 외곽 꼭짓점 중 가장 낮은 지형 높이. 면 색은 MapLibre 조명식(기본 방향·세기 0.5, 벽은 MapLibre처럼 안쪽 법선으로 계산해 남남서 벽이 밝음, 위쪽 벽 기울기 계수는 벽 단위 평균)을 지도에 고정한 빛으로 미리 계산. 높이가 상수 0이면 평면 채우기로 둠. CPU 래스터(`render/extrusion.rs`)는 2×2 표본 깊이 버퍼로 레이어를 불투명하게 그린 뒤 레이어 투명도로 섞어(겹친 면이 비치지 않음) 지형 깊이 맵과 비교하고, 건물 깊이를 깊이 맵에 더해 트랙 가림(#39)이 건물에도 적용됨. 출력 32행 띠 단위로 표본 버퍼를 제한, 행마다 삼각형이 덮는 구간을 구해 깊이를 증분, 카메라 반대쪽 벽은 생략. 3D 지면 지도·지형 텍스처는 extrusion 규칙을 빼고 그림. 서울 z16 720p 120프레임 8스레드 4.7 → 7.4s(건물 약 3만 개, 프레임당 단일 스레드 약 55ms) (#43)
- [x] 3D 건물(브라우저): 지면 타일 워커(`GroundTiles`)가 텍스처는 extrusion 규칙을 빼고 그리고, 같은 지면 타일 사각형으로 자른 건물 면을 타일 기준 좌표·건물 번호·색과 함께 넘김(`buildings`, 오버줌 타일도 자기 몫만). 메인 스레드 `SceneRenderer.glBuildings`가 건물마다 바닥(아래 가장 낮은 지형)을 구해 장면 좌표로 바꾸고, three.js는 지면 타일과 함께 보이고 숨는 메시로 깊이만 먼저 쓴 뒤(불투명) 투명도로 색을 섞어 CPU와 같이 겹친 면이 비치지 않고 트랙이 건물 뒤에서 가려짐. 바닥은 지형 유니폼(`uTerrain`)으로 함께 눕고, 먼 곳은 지면처럼 배경색으로 페이드. 남산 liberty 궤도 회전(라벨·지형 켬) 52.8 → 51.2fps (#43)
- [x] 브라우저 2D 지도를 워커로: 2D 지도의 MVT 디코드·지도·라벨 그리기를 전용 워커의 `GpxRenderer`로 옮기고, 메인 스레드는 타일을 받아 넘기고(`Tile` 생성자로 배치 복원) 결과 RGBA를 `putImageData`만 함. 라벨·keepout 변경 중 그리기가 진행 중이면 다음 한 번으로 합침. 워커가 실패하면 받은 타일·설정을 메인 스레드 `GpxRenderer`에 다시 적용해 같은 그림. 로드 중 100ms 넘는 작업 2회(`renderMap` 각 약 205ms: 타일 로드 후, 라벨 폰트 도착 후) → 0회(남산 샘플, 프로덕션 빌드) (#45)

---

## 8. 결정해야 할 것 (Open Questions)

- **타일 소스 라이선스**: OSM 타일 서버는 대량 요청 시 정책 위반 소지. 현재는 기본값 없이 사용자가 지정한다. 벡터 지도(Milestone 6)는 OpenFreeMap처럼 키 없이 쓸 수 있는 소스가 있고, 오프라인 PMTiles가 붙으면 네트워크 정책 문제가 사라진다.
- **병렬 렌더 vs 스트리밍 인코딩**: rayon 병렬 렌더는 프레임을 메모리에 모아야 하므로 긴 영상에서 메모리 압박. 청크 단위 병렬 + 순차 flush 전략 설계 필요.
- **ffmpeg 의존성**: 외부 프로세스로 시작하되, 자급자족 바이너리가 목표라면 `ffmpeg-next` FFI 전환 시점 결정.
- **좌표 보간 방식**: 단순 선형 vs Catmull-Rom 스플라인(부드러운 곡선). GPS 노이즈 스무딩 여부.

---

## 9. 참고

- 원본: [gpx-animator/gpx-animator](https://github.com/gpx-animator/gpx-animator) (Java, Apache-2.0)
- 라이선스 확인: 포팅 시 원본 라이선스 조건 검토 필요 (Apache-2.0이면 attribution 유지)
- Web Mercator 변환: [OSM Slippy Map Tilenames](https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames)

---

## 10. 첫 작업 지시 (Claude Code용)

> Milestone 1부터 시작한다. 먼저 `cargo init`으로 프로젝트를 만들고
> `gpx/model.rs`의 `TrackPoint` 구조체와 `gpx/parser.rs`의 `quick-xml` 기반 파서,
> `geo/mercator.rs`의 좌표 변환 함수를 작성한 뒤,
> 샘플 GPX를 단색 배경 위 트랙 폴리라인 PNG로 출력하는 것까지 구현한다.
> 각 모듈에 단위 테스트를 함께 작성한다.
