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

- [ ] GPX 파싱 (`gpx/parser.rs`)
- [ ] 좌표 변환 (`geo/mercator.rs`)
- [ ] 단색 배경 + 트랙 폴리라인을 PNG 한 장으로 출력 (지도 타일 없이)
- ✅ 산출물: `track.png`

### Milestone 2 — 점 이동 영상

- [ ] 시간 보간 + 프레임 루프
- [ ] ffmpeg rawvideo 파이프 (`encode/ffmpeg.rs`)
- [ ] tail(꼬리) 렌더
- ✅ 산출물: 배경 없는 점 이동 MP4

### Milestone 3 — 지도 배경

- [ ] TMS 타일 fetch + 캐시 (`tiles/fetcher.rs`)
- [ ] 뷰포트 타일 합성 (`tiles/composer.rs`)
- [ ] zoom 자동 계산
- ✅ 산출물: 지도 위 트랙 애니메이션 MP4

### Milestone 4 — 디테일

- [ ] pre-draw-track (전체 경로 사전 렌더)
- [ ] 정보 오버레이 (lat/lng, speed, time)
- [ ] attribution 표시/숨김
- [ ] 카메라 이동 (뷰포트 < 트랙)
- [ ] 복수 GPX 입력

### Milestone 5 — 성능 & 배포

- [ ] rayon 프레임 병렬화
- [ ] 타일 캐시 디스크 영속화 (24h TTL)
- [ ] (옵션) napi-rs Node 바인딩
- [ ] CI 빌드 (멀티플랫폼 단일 바이너리)

---

## 8. 결정해야 할 것 (Open Questions)

- **타일 소스 라이선스**: OSM 타일 서버는 대량 요청 시 정책 위반 소지. 기본값을 OSM으로 둘지, 사용자가 반드시 지정하게 할지. 오프라인 `mbtiles`/`pmtiles` 지원 우선순위.
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
