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
- [x] 영상 중 카메라 zoom 애니메이션: `--zoom-transition`으로 시작·끝에 전체 경로 ↔ 추적 zoom 이징. 2D는 zoom을 선형, 중심을 두 화면이 공유하는 고정점 기준으로 보간(순수 줌), 3D는 추적 zoom의 월드에서 거리·조준점을 같은 방식으로 보간해 지형과 함께 동작 (#23). 중간 zoom 프레임은 2D 추적처럼 순차 렌더, 3D는 `Camera::detail_zoom`(양 끝 zoom 사이 선형)으로 도로 폭·타일 zoom을 정해 줌 아웃 화면이 일반 3D와 같은 세부를 가짐. 남은 한계: 타일 zoom 경계에서 지도 세부가 바뀌는 순간, 지형은 추적 zoom으로 경로 주변만 만들어 줌 아웃 시 지형 섬과 축소 텍스처 반짝임(밉맵 없음)
- [x] 오프라인 PMTiles 소스: 코어에 v3 헤더·디렉터리·Hilbert tile id·gzip 해제, CLI 로컬 `.pmtiles`, HTTP MVT gzip 자동 해제 (#7)
- [ ] 브라우저/원격 PMTiles(HTTP Range), brotli·zstd 압축
- [x] 사용자 스타일 파일: 자체 JSON(`extends`, `background`, `rules`), CLI 경로·브라우저 문자열, 예시 `styles/` (#8)
- [ ] MapLibre style JSON 부분 가져오기
- [x] 라벨: 사용자/시스템 한글 폰트, place·peak·water_name 점 라벨, 중요도순 탐욕 배치(라벨·트랙·오버레이 회피), halo, 3D 깊이 축소 (#9, CLI)
- [x] 브라우저 라벨: JS가 폰트 바이트(NanumGothic)를 전달, 2D 지도와 3D 장면 모두. 3D는 라벨 레이어만 디코드(`VectorTile::decode_layers`)해 키프레임 워커와 독립, 키프레임별로 라벨 후보를 캐시(`LabelSources`)하고 방위마다 화면에서 배치해 지면 캐시와 함께 보관. 라벨 배치·3D 커튼 외곽선·대비 색상을 core로 옮겨 CLI와 공유, 후광은 글리프 외곽선을 한 번만 읽음 (#27). 회전 중 실측 42km에서 ~30 → ~25fps
- [ ] 도로명 등 선형 라벨, 복잡한 스크립트 셰이핑, 3D 라벨 프레임 간 안정화(히스테리시스), 지형에 의한 라벨 가림, 브라우저 HTML 오버레이(출처 표기)와의 겹침

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
- [x] 브라우저 연속 궤도 회전: 지면을 10° 키프레임으로만 정확히 렌더하고, 사이 방위는 두 키프레임을 호모그래피로 재투영 (#3). 회전 중 ~9fps → ~30fps(실측 42km 트랙, 키프레임 캐시 후). 키프레임 첫 방문 멈춤(~220ms)은 Web Worker 렌더로 제거: 현재 방위에 가까운 키프레임부터 워커가 그려 전달하고, 도착 전에는 가장 가까운 캐시 키프레임으로 재투영 (#20). 15초 자동 회전에서 100ms 넘는 프레임 17회 → 0회
- [x] 원근에 따른 도로 폭 보정(깊이별 1/8 단계 배율), 3D 꼬리 페이드(`--tail-duration`) (#2)
- [x] 마커 진행 방향을 따르는 추적 카메라: 거리 창 chord 방위 + 프레임당 회전 속도 제한 (#1)
- [x] 지형(DEM) 기반 지면 기복: Terrarium 타일, 경로 주변 지도 텍스처 + hillshade를 삼각형 메시에 매핑(화가 알고리즘), 가장자리 높이 감쇠, 트랙·커튼·그림자를 지형 위에 (#4, CLI)
- [x] 브라우저 지형: Terrarium 타일을 JS가 디코드해 전달, 지도 텍스처는 키프레임 워커가 그림(`renderTerrainTexture`), 키프레임은 평면 지도 그대로 재투영하고 그 위에 지형 메시를 방위마다 정확히 그려 지면 캐시에 함께 보관 (#21). 지형 삼각형을 직접 스캔라인으로 채움: 꼭짓점을 1/256px로 맞춰 정수 에지 함수로 판정(공유 에지·꼭짓점의 픽셀이 정확히 한 삼각형에 속함), 텍스처는 고정소수점 bilinear. tiny-skia 패턴 대비 640px 10.6 → 6.3ms, CLI 출력 PSNR 69.5dB. 텍스처 최대 2048px(메인 16MB, 워커에 그릴 때 16MB 추가). 회전 중 실측 42km에서 라벨+지형 ~20fps
- [ ] 지형에 의한 트랙 가림, 성긴 트랙 구간이 언덕을 관통하는 문제(정점 사이 직선), 카메라가 높은 지형보다 낮을 때의 최소 고도(브라우저)

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
