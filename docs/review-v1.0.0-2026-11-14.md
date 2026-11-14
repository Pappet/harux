# Harux — Review & Performance-Test v1.0.0 (2026-11-14)

> **Momentaufnahme.** Dieses Dokument beschreibt den Stand von v1.0.0
> (Commit `7da204c`) am 14.11.2026. Dateinamen, Zeilennummern und Messwerte
> gelten für diesen Stand. Für die aktuellen Regeln siehe
> [`AGENTS.md`](../AGENTS.md), die [Roadmap](ROADMAP.md) und das
> [Changelog](../CHANGELOG.md).

> Review der Änderungen von Milestone 4 (Senden, Drafts, Templates, Runs,
> Import, Watch) und Lasttest des Release-Builds, verglichen mit v0.9.0.
>
> Alle Befunde sind als GitHub-Issues erfasst (#298–#307) und unten verlinkt.

## Inhaltsverzeichnis

- [Ergebnis](#ergebnis)
- [Umfang und Methode](#umfang-und-methode)
- [1. Befunde](#1-befunde)
- [2. Performance-Messungen](#2-performance-messungen)
- [3. Beobachtungen ohne Issue](#3-beobachtungen-ohne-issue)
- [4. Was sich bestätigt hat](#4-was-sich-bestätigt-hat)
- [5. Empfohlene Reihenfolge](#5-empfohlene-reihenfolge)
- [Befund-Index → Issues](#befund-index--issues)

---

## Ergebnis

v1.0.0 ist solide. Die CI-Prüfungen sind auf dem Release-Stand grün, der
Empfang ist gegenüber v0.9.0 nicht langsamer geworden, und die neuen Wege
(Senden, Drafts, Import) sind schnell.

Der Review ergab **zehn Befunde**: einen schweren (das Speicherlimit lässt
sich über einen Run mit Änderungen umgehen), vier mittlere und fünf kleine.
Acht davon sind gemessen oder im Browser nachgestellt, zwei (B-9, B-10) sind
am Code belegt.

Mit `send.enabled = false` — dem Standard — betrifft davon nur B-9 eine
Installation; alle anderen setzen eingeschaltetes Senden voraus.

---

## Umfang und Methode

**Review.** Der Diff `f157278..7da204c`: zwölf Pull Requests (#284–#297),
76 Dateien, 17.284 eingefügte Zeilen. Jeder Befund wurde am Code gegengelesen
und, wo es möglich war, gemessen oder in einem echten Browser nachgestellt.
Bei jedem Befund steht, wie er belegt ist.

**CI-Prüfungen** auf `7da204c`: `cargo fmt --check`, `cargo clippy -- -D
warnings`, `cargo test` und die 113 JS-Tests laufen fehlerfrei durch.

**Performance-Test.** Release-Builds von v0.9.0 (`eeb7955`) und v1.0.0
(`7da204c`), Standardkonfiguration plus `send.enabled = true`:

- **Lastgenerator** (eigenes Programm, nur Rust-Standardbibliothek): hält
  MLLP-Verbindungen offen, sendet eine Nachricht, wartet auf ihr ACK, misst
  die Zeit dazwischen. Nachrichtenarten: ADT (ca. 400 Bytes), ORU mit 20 OBX
  (ca. 2 KB), MDM mit Base64-Anhang in wählbarer Größe, und ein Mix aus 70 %
  ADT, 25 % ORU und 5 % MDM mit 200 KB.
- **Senke:** ein lokaler MLLP-Empfänger, der jede Nachricht sofort mit `AA`
  quittiert — das Ziel für Sends und Runs.
- **Sonde:** eine eigene Verbindung, die alle 5 ms einen ADT sendet und die
  Zeit bis zum ACK misst, während daneben Last läuft.
- **API:** sequenzielle Anfragen über eine Keep-Alive-Verbindung.
- **Oberfläche:** Chromium headless (Playwright); gemessen wurden Long Tasks,
  die Abstände zwischen Frames und einzelne Abläufe.

**Grenzen der Messung.** Eine Linux-VM mit 4 vCPUs und 15 GB RAM, alles über
localhost. Server, Lastgenerator und Browser teilen sich die Kerne, und die
Produktion läuft auf Windows Server. Die absoluten Zahlen sind deshalb nicht
übertragbar; die Vergleiche (v0.9.0 gegen v1.0.0, vorher gegen nachher) sind
es. Nicht gemessen: Senden über TLS unter Last, Windows, andere Browser als
Chromium.

---

## 1. Befunde

| Nr. | Schwere | Befund | Issue | Beleg |
|---|---|---|---|---|
| B-1 | Hoch | Run-Grenzen greifen erst, nachdem alle geänderten Nachrichten gebaut sind | [#298](https://github.com/Pappet/harux/issues/298) | gemessen |
| B-2 | Mittel | Klick auf „Send" geht nach einer Feldänderung verloren | [#299](https://github.com/Pappet/harux/issues/299) | nachgestellt |
| B-3 | Mittel | Behaltene Drafts werden von einem zweiten Tab gelöscht | [#300](https://github.com/Pappet/harux/issues/300) | nachgestellt |
| B-4 | Mittel | Draft-Vorschau und Build rechnen auf den Async-Workern und bremsen ACKs | [#301](https://github.com/Pappet/harux/issues/301) | gemessen, Gegenprobe |
| B-5 | Mittel | Runs sind auf ca. 700 Nachrichten/s gedrosselt | [#302](https://github.com/Pappet/harux/issues/302) | gemessen, Gegenprobe |
| B-6 | Niedrig | Feldänderung geht still verloren, wenn man während des Schreibens wechselt | [#303](https://github.com/Pappet/harux/issues/303) | nachgestellt |
| B-7 | Niedrig | Diff-Tab fordert die gefüllte Vorschau doppelt an | [#304](https://github.com/Pappet/harux/issues/304) | nachgestellt |
| B-8 | Niedrig | `{{now}}` trägt im ersten Durchlauf eines Runs die Anfragezeit | [#305](https://github.com/Pappet/harux/issues/305) | nachgestellt |
| B-9 | Niedrig | `403` bei ausgeschaltetem Senden kommt erst, nachdem der Body gelesen ist | [#306](https://github.com/Pappet/harux/issues/306) | Code |
| B-10 | Niedrig | Log enthält den Anfang einer Antwort, die kein HL7 ist | [#307](https://github.com/Pappet/harux/issues/307) | Code |

### B-1 — Run-Grenzen greifen erst nach dem Bauen: `store.max_memory_mb` ist umgehbar

[Issue #298](https://github.com/Pappet/harux/issues/298) ·
`src/send.rs:532–563`

`Sender::start` ruft `prepare` auf, das die erste Kopie **jeder** Nachricht
baut, die nicht unverändert gesendet wird (gespeicherte Nachricht mit `set`
oder Draft), und alle Kopien hält. Erst danach wird die Größe des Runs mit
`max_run_bytes` verglichen und der Run-Permit geholt. Die beiden Grenzen, die
einen Run beschränken sollen, greifen also, nachdem der Speicher verbraucht
ist — und beliebig viele solcher Anfragen bauen gleichzeitig. Der Body ist
klein (nur IDs), ein Large-Body-Permit greift daher auch nicht.

Gemessen mit einer MDM im Store, die N-mal mit `"set": {"MSH-10": "{{id}}"}`
angefragt wird:

| Anfrage | Body | Antwort | Dauer | RSS vorher → Spitze |
|---|---|---|---|---|
| 100 × MDM 2 MB | 5,6 KB | `413` | 1,09 s | 12 MB → 394 MB |
| 400 × MDM 2 MB | 22 KB | `413` | 5,03 s | 12 MB → 1.546 MB |
| 100 × MDM 8 MB | 5,6 KB | `413` | 4,79 s | 23 MB → 1.567 MB |
| dieselben ohne `set` | | `413` | 1–7 ms | unverändert |

Nach der Ablehnung blieb der RSS auf der Spitze. Erlaubt sind 1000 Items mit
bis zu 10 MB, eine Anfrage kann also weit über 15 GB anfordern, bevor sie
abgelehnt wird. Über die Oberfläche ist das erreichbar: *Resend filtered* mit
*With a new control ID* auf einer Liste mit vielen MDM-Nachrichten.

**Empfehlung:** die Größe vor dem Bauen aus den gespeicherten Nachrichten
schätzen und ablehnen, den Permit vor `prepare` holen, und die Kopien erst
beim Senden bauen (das löst auch B-8).

### B-2 — Klick auf „Send" geht nach einer Feldänderung verloren

[Issue #299](https://github.com/Pappet/harux/issues/299) ·
`static/compose.js:1209–1210`, `868–879`

Nach jedem Feld-Schreiben im Segments-Tab ersetzen `renderDraftRail()` und
`renderDraftHeader()` (über `updateDraftActions`) die Buttons per `innerHTML`.
Der Klick auf **Send** beendet die Eingabe: Maus-runter löst das Schreiben
aus, die Antwort ist im LAN vor Maus-hoch da, der Button ist dann ein anderer
— der Browser löst keinen `click` aus.

Nachgestellt (Feld PID-5 ändern, Maus 80 ms auf **Send** halten):

| `/api/parse` antwortet | Sende-Anfragen |
|---|---|
| sofort (localhost) | 0 |
| nach 30 ms | 0 |
| nach 300 ms (nach Maus-hoch) | 1 |
| Kontrolle: kein Feld geändert | 1 |

Im LAN braucht „Feld ändern, Send" also praktisch immer zwei Klicks.

**Empfehlung:** Elemente nicht ersetzen, deren Markup sich nicht geändert
hat; den einen Chip der Draft-Leiste an Ort und Stelle aktualisieren.

### B-3 — Behaltene Drafts werden von einem zweiten Tab gelöscht

[Issue #300](https://github.com/Pappet/harux/issues/300) ·
`static/compose.js:102–125`

Ein Tab liest `harux_drafts_kept` einmal beim Laden. `saveDrafts` schreibt
danach bei jeder Änderung die ganze Liste aus dem Speicher dieses Tabs;
niemand hört auf `storage`-Ereignisse. Wer zuletzt speichert, gewinnt — mit
der Liste, die er beim Öffnen hatte.

Nachgestellt mit zwei Tabs: Tab A behält einen Draft (`localStorage`: 1), Tab
B tippt ein Zeichen in einen eigenen Draft (`localStorage`: 0), nach dem
Neuladen von A ist der Draft weg. Die Zielliste (`saveTargets`,
`static/send.js`) folgt demselben Muster; sie wurde nur gelesen, nicht
nachgestellt.

**Empfehlung:** vor dem Schreiben mit dem Stand im Storage nach Draft-ID
zusammenführen und auf `storage`-Ereignisse hören.

### B-4 — Draft-Vorschau und Build rechnen auf den Async-Workern

[Issue #301](https://github.com/Pappet/harux/issues/301) ·
`src/web.rs:739, 764, 912`, `src/send.rs:677`

`parse_draft`, `build_draft` und der Draft-Zweig von `Sender::resolve`
parsen, bauen und serialisieren direkt in der async-Funktion.
`message_draft` in derselben Datei nutzt dafür `spawn_blocking`. Solange ein
Worker so beschäftigt ist, warten die ACKs des Listeners — auch wenn die
anderen Worker frei sind. Die Messwerte stehen in
[Abschnitt 2.6](#26-ack-latenz-des-listeners-unter-last): Eine einzelne
gefüllte Vorschau einer 8-MB-MDM (was der Diff-Tab anfordert) hebt die
ACK-Latenz von p99 0,63 ms auf 97 ms; mit `spawn_blocking` sind es 5,5 ms.

Normales Tippen ist harmlos (eine Vorschau dauert 0,2–0,5 ms). Teuer sind
große Texte: Drafts aus Dateien, ORU mit tausenden OBX und die gefüllte
Vorschau einer MDM. Das Muster gab es schon vor Milestone 4 (Detail-JSON,
Empfang großer Nachrichten); M4 bringt den schwersten Fall dazu.

Eine gefüllte Vorschau nimmt außerdem keinen Large-Body-Permit, weil ihr Body
klein ist, baut aber für eine 8-MB-MDM rund 30 MB JSON. Im Messlauf mit vier
solchen Anfragen parallel (neben Detail- und Build-Abrufen) erreichte der
Prozess rund 1 GB RSS bei einem Store von etwa 30 MB.

**Empfehlung:** die drei Stellen samt Serialisierung in `spawn_blocking`
ausführen; der Versuchs-Code steht im Issue.

### B-5 — Runs sind auf ca. 700 Nachrichten/s gedrosselt

[Issue #302](https://github.com/Pappet/harux/issues/302) ·
`src/mllp_client.rs:394–406`

Vor jeder Nachricht auf einer gehaltenen Verbindung prüft
`Connection::is_reusable` mit `timeout(Duration::ZERO, tcp.peek(…))`, ob das
Ziel sie geschlossen hat. Der Null-Timeout endet nicht „sofort": Er wartet
auf den nächsten Tick des Tokio-Timers, etwa eine Millisekunde — einmal pro
Nachricht.

| | Run 1000 unverändert | Run 1000 mit `MSH-10`/`MSH-7` |
|---|---|---|
| v1.0.0 | 1,46–1,51 s (663–686/s) | 1,42 s (706/s) |
| ein `poll_peek` ohne Timer (Versuch) | 0,13–0,14 s (7.114–7.830/s) | 0,08 s (12.024/s) |

Mit der Änderung laufen alle 39 Tests in `tests/send_tcp.rs` durch. Bei
großen Nachrichten spielt der Tick keine Rolle.

### B-6 — Feldänderung geht still verloren, wenn man währenddessen wechselt

[Issue #303](https://github.com/Pappet/harux/issues/303) ·
`static/compose.js:1170–1213`

Ein Feld-Schreiben ist an den Draft gebunden, der beim Ausführen und bei der
Antwort offen ist — nicht an den, dessen Feld geändert wurde. Wechselt man
vor der Antwort zu einer Nachricht oder einem anderen Draft, wird der Wert
verworfen (`return false` in Zeile 1195), ohne Hinweis.

Nachgestellt: Bei sofortiger Antwort bleibt die Änderung erhalten, bei 300 ms
Antwortzeit fehlt sie im Draft. Solche Zeiten entstehen von selbst bei großen
Drafts (gemessen: 205 ms mit 2-MB-Anhang im Text, 550 ms bei 2000 OBX). Aus
dem Code folgt ein zweiter Fall, der nicht nachgestellt wurde: Ein noch
wartender Schreibvorgang würde nach dem Wechsel in den **anderen** Draft
schreiben.

### B-7 — Diff-Tab fordert die gefüllte Vorschau doppelt an

[Issue #304](https://github.com/Pappet/harux/issues/304) ·
`static/compose.js:899–903`, `911–915`

`renderDraftTab` ruft beim Wechsel in den Diff-Tab an zwei Stellen
`requestDraftPreview(d, true)` auf; beide Bedingungen treffen gleichzeitig
zu. Nachgestellt mit dem Draft einer 8-MB-MDM: zwei `POST /api/parse` mit
`fill_references`, je 164 ms Serverzeit und rund 30 MB Antwort — genau die
Anfrage, die nach B-4 die ACKs bremst.

### B-8 — `{{now}}` trägt im ersten Durchlauf die Anfragezeit

[Issue #305](https://github.com/Pappet/harux/issues/305) ·
`src/send.rs:898`, `1249–1253`

Die erste Kopie jedes Items wird beim Eingang der Anfrage gebaut und später
an ihrer Position gesendet. Nachgestellt mit drei Nachrichten, `interval_ms:
1500` und `repeat: 2`: Die ersten drei tragen in MSH-7 alle `…091911`,
gesendet wurden sie um :11, :12 und :14. Der zweite Durchlauf trägt die
jeweils aktuelle Zeit.

### B-9 — `403` kommt erst, nachdem der Body gelesen ist

[Issue #306](https://github.com/Pappet/harux/issues/306) ·
`src/web.rs:549, 727, 891`

„`403`, solange Senden aus ist" steht als Zeile am Anfang von zehn Handlern.
Bei `/api/send`, `/api/parse` und `/api/build` hat axum den Body (bis 21 MB)
dann schon gelesen. Begrenzt durch die vier Large-Body-Permits auf 4 × 21 MB.
Eine künftige Route, die die Zeile vergisst, wäre bei ausgeschaltetem Senden
erreichbar. **Empfehlung:** ein Guard als Layer vor den Sende-Routen.

### B-10 — Log enthält den Anfang einer Nicht-HL7-Antwort

[Issue #307](https://github.com/Pappet/harux/issues/307) ·
`src/send.rs:1058–1070`, `src/hl7/parser.rs:36–40`

`Sender::record` loggt für einen fehlgeschlagenen Send `error_detail`. Parst
die Antwort des Ziels nicht, enthält dieser Text die ersten 20 Zeichen der
Antwort — das widerspricht Senderegel 7 in `AGENTS.md` („Log lines carry
metadata only, never content"). Der Listener loggt denselben Text für
empfangene Frames schon länger (`src/mllp.rs:325`).

---

## 2. Performance-Messungen

Zeiten sind, wenn nicht anders genannt, p50 / p99. Jedes Szenario lief auf
einem frisch gestarteten Server.

### 2.1 Empfang: v0.9.0 gegen v1.0.0

| Szenario | v0.9.0 | v1.0.0 |
|---|---|---|
| ADT, 1 Verbindung, 50.000 Nachrichten | 14.800/s · ACK 0,06 / 0,11 ms | 15.200/s · 0,06 / 0,11 ms |
| ADT, 8 Verbindungen, 200.000 | 37.700/s · p99 1,5 ms | 38.300/s · p99 1,5 ms |
| Mix (5 % MDM 200 KB), 8 Verbindungen, je 3 Läufe | 21.500–22.300/s | 20.600–22.600/s |
| MDM 2 MB, 4 Verbindungen | 300/s · 571 MB/s · 8 / 18 ms | 299/s · 571 MB/s · 8 / 19 ms |
| MDM 8 MB, 2 Verbindungen | 32/s · 243 MB/s · p50 40 ms | 30/s · 231 MB/s · p50 39 ms |

**Keine Regression.** Die Unterschiede liegen im Rauschen der Messung.

### 2.2 Senden und Runs

Serverzeit einschließlich der HTTP-Anfrage, Ziel ist die lokale Senke.

| Vorgang | Ergebnis |
|---|---|
| Resend unverändert | 0,79 / 2,5 ms |
| Resend mit neuer Control-ID und Zeit | 0,72 / 3,5 ms |
| Resend an *This Harux* | 0,71 / 2,7 ms |
| Draft senden | 0,83 / 1,7 ms |
| Resend MDM 2 MB: unverändert / geändert / Draft mit Referenz | 5 / 17 / 13 ms |
| Resend MDM 8 MB: unverändert / geändert / Draft mit Referenz | 20 / 72 / 55 ms |
| Run 1000 × ADT unverändert | 1,46 s (680/s) — siehe B-5 |
| Run 30 × MDM 2 MB | 0,12 s (510 MB/s) |
| Run 40 × MDM 2 MB | `413` (153 MB > 25 % des Stores) — wie vorgesehen |

Die 153 MB sind die im Store **verbuchte** Größe, nicht die Bytes auf der
Leitung: Eine 2-MB-MDM wird mit rund 3,8 MB verbucht (etwa das 1,9-Fache),
weil ihr Anhang in `raw` und im Feldwert steht (`estimated_bytes`). Deshalb
passen 30 solcher Nachrichten in die 128 MB eines Runs, 40 nicht.

### 2.3 Drafts: Vorschau, Draft-Text, Build

| Vorgang | Zeit | Antwortgröße |
|---|---|---|
| Vorschau ADT (5 Segmente) | 0,21 / 0,53 ms | 10 KB |
| Vorschau ORU (25 Segmente) | 0,45 / 0,68 ms | 37 KB |
| Vorschau ORU mit 200 OBX (14 KB Text) | 1,8 / 3,4 ms | 322 KB |
| Vorschau ORU mit 2000 OBX (142 KB Text) | 16 / 26 ms | 3,2 MB |
| Draft-Text einer gespeicherten MDM 8 MB | 12 ms | 516 Bytes |
| Vorschau dieses Drafts (Referenz bleibt) | 0,19 ms | 7,8 KB |
| Vorschau gefüllt (Diff-Tab), MDM 2 MB / 8 MB | 30 ms / 164 ms | 7,6 MB / 30,5 MB |
| Build (*Save as file*), MDM 2 MB / 8 MB | 5 ms / 19 ms | 1,9 MB / 7,6 MB |

### 2.4 Import

| Datei | Dauer | Durchsatz |
|---|---|---|
| 10.000 ADT (3,9 MB) | 0,27 s | 37.600 Nachrichten/s |
| 200.000 ADT (78 MB) | 4,98 s | 40.200 Nachrichten/s |
| Mix, 20.000 Nachrichten (205 MB) | 1,85 s | 110 MB/s |
| 150 × MDM 2 MB (286 MB) | 1,71 s | 167 MB/s |

Während des Imports von 200.000 ADT blieb der Store im Limit (höchstens
10.066 Nachrichten, 88 MB).

### 2.5 API und WebSocket bei vollem Store

Store mit 9.987 Nachrichten, 346 MB verbucht.

| Anfrage | Zeit |
|---|---|
| `GET /api/stats` | 0,12 / 0,17 ms |
| Liste, 100 Einträge | 0,38 / 0,69 ms |
| Liste, 1000 Einträge mit Lesezeichen (Seitenaufbau) | 4,3 / 6,0 ms |
| Suche mit 500 Treffern | 2,0 / 2,3 ms |
| Suche ohne Treffer (ganzer Store) | 3,7 / 6,4 ms |
| Detail ADT / MDM 200 KB | 0,18 / 0,79 ms (p50) |
| Export, 102 MB | 0,13 s |

WebSocket: Mit 10 verbundenen Clients verarbeitete der Server unter Flut
17.900 Nachrichten/s, mit 30 Clients 10.400/s. Jeder Client erhielt alle
200.000 Ereignisse, kein `lagged`.

### 2.6 ACK-Latenz des Listeners unter Last

Die Sonde sendet 8 s lang alle 5 ms einen ADT; daneben wiederholt ein Client
eine Anfrage. Werte: p50 / p95 / p99 / max.

| Last neben der Sonde | v1.0.0 | Vorschau in `spawn_blocking` (Versuch) |
|---|---|---|
| keine | 0,27 / 0,42 / 0,63 / 2,7 ms | 0,28 / 0,40 / 0,71 / 3,3 ms |
| gefüllte Vorschau MDM 8 MB | 0,29 / 82 / 97 / 129 ms | 0,26 / 2,5 / 5,5 / 8,6 ms |
| dieselbe, vier Clients | 0,23 / 29 / 66 / 99 ms | 0,20 / 5,1 / 9,4 / 16,5 ms |
| gefüllte Vorschau MDM 2 MB | 11 / 18 / 21 / 24 ms | 0,25 / 0,36 / 0,57 / 9 ms |
| Vorschau ORU mit 2000 OBX | 9,9 / 18 / 21 / 43 ms | 0,25 / 0,37 / 0,65 / 15 ms |
| Vorschau ORU mit 25 Segmenten, vier Clients (2.600/s) | 0,18 / 0,83 / 1,5 / 2,7 ms | 0,14 / 0,47 / 0,71 / 2,3 ms |
| `/api/build` MDM 8 MB | 23 / 30 / 38 / 43 ms | — |
| Draft-Text MDM 8 MB (`spawn_blocking`) | 0,25 / 0,37 / 0,84 / 9 ms | — |
| Runs mit je 980 geänderten Nachrichten | 0,23 / 0,40 / 0,99 / 2,7 ms | — |
| Resend MDM 8 MB mit neuer ID | 0,26 / 0,54 / 2,3 / 11,6 ms | — |
| Detail-JSON MDM 8 MB (vor M4) | 2,8 / 23 / 28 / 46 ms | — |
| ein MLLP-Sender mit MDM 8 MB (vor M4) | 0,27 / 37 / 43 / 48 ms | — |

Runs und Resends stören den Empfang nicht. Auffällig sind nur die Wege, die
große Texte auf einem Async-Worker verarbeiten (B-4).

### 2.7 Oberfläche

| Szenario | Ergebnis |
|---|---|
| Seitenaufbau, 10.000 Nachrichten im Store | Liste nach 0,1–0,3 s |
| Live 200 Nachrichten/s, 10 s | 3 Long Tasks, längster 66 ms |
| Live 2.000 Nachrichten/s, 10 s | 1 Long Task (59 ms), Frames p99 49 ms |
| Flut (ca. 19.000/s am Server) | Long Tasks bis 211 ms, Frames p99 164 ms — bedienbar |
| Flut mit aktivem Suchfilter | Long Tasks bis 152 ms |
| Flut bei pausierter Liste | Long Tasks bis 100 ms |
| Flut mit Watch, Tab im Hintergrund | Long Tasks bis 282 ms |
| Run mit 1000 Nachrichten | höchstens 1 Long Task (57 ms); alle Ergebnisse nach 2,1 s in der Liste |

Draft-Editor:

| Draft | Tastendruck | Vorschau-Rundlauf | Segments-Tab | Feld schreiben |
|---|---|---|---|---|
| ADT, 5 Segmente | 0,2 ms | 5 ms | 19 ms (34 Felder) | 20 ms |
| ORU, 25 Segmente | 0,2 ms | 5 ms | 34 ms (208 Felder) | 23 ms |
| ORU, 200 OBX | 0,2 ms | 12 ms | 287 ms (1.828 Felder) | 42 ms |
| ORU, 2000 OBX | 0,3 ms | 54 ms | 2,2 s (18.028 Felder) | 550 ms |
| MDM-Datei, 2 MB Anhang im Text | 0,5 ms | 198 ms | 28 ms | 205 ms |

### 2.8 Speicher

| Szenario | RSS |
|---|---|
| 200.000 ADT empfangen | 117 MB |
| Mix-Flut (80.000 Nachrichten) | ca. 410 MB |
| MDM-Flut 2 MB bei 512-MB-Limit | Spitze 617 MB (v0.9.0) / 648 MB (v1.0.0) |
| Import 150 × MDM 2 MB | Spitze 583 MB |
| abgelehnter Run nach B-1 | bis 1,5 GB, bleibt belegt |

Dass der RSS das Store-Limit um den Parse-Baum der Frames in Arbeit und um
vom Allokator gehaltenen Speicher übersteigt, ist in `AGENTS.md` beschrieben
und in beiden Versionen gleich.

---

## 3. Beobachtungen ohne Issue

- **Segments-Tab bei sehr großen Nachrichten.** Ein ORU mit 2000 OBX erzeugt
  18.000 Eingabefelder und 132.000 DOM-Knoten; der Tab braucht 2,2 s. Für
  übliche Nachrichten kein Thema.
- **Vorschau-Antwort ist ein Vielfaches des Drafts.** Für 142 KB Text kommen
  3,2 MB JSON zurück (Text, Nachricht mit `raw`, Felder mit Beschreibung und
  Struktur, Positionen) — nach jeder Tipp-Pause. Bei üblichen Nachrichten
  sind es 10–40 KB.
- **Draft aus einer Datei hält den Anhang im Text.** Referenzen
  (`{{from:…}}`) entstehen nur aus gespeicherten Nachrichten. Eine MDM-Datei
  mit 2 MB Anhang öffnet in 1,3 s und bleibt bedienbar (Vorschau 0,2 s).
- **Wiederholung nach einem Hangup.** Schließt das Ziel eine gehaltene
  Verbindung ohne Antwort, schreibt `exchange_on` die Nachricht einmal neu.
  Hatte das Ziel sie schon gelesen, kommt sie doppelt an. Das ist in
  `AGENTS.md` als bewusste Entscheidung dokumentiert (Empfänger, die nach
  jedem ACK schließen, sind häufig).
- **Gesendete Nachrichten stehen ein zweites Mal im Store.** Vier Runs
  gleichzeitig dürfen je 25 % belegen und können so den empfangenen Bestand
  verdrängen — so vorgesehen, aber gut zu wissen.
- **Watch-Zähler unter Flut.** Von 200.000 Nachrichten zählte der Tab 64.249,
  weil Nachrichten nach einem `lagged` mit der neu geladenen Liste kommen.
  Das ist dokumentiert („a nudge, not a tally").
- **Vor M4 vorhandene Muster.** Das Detail-JSON einer 8-MB-MDM ist 23 MB
  groß, und der Listener parst große Frames auf dem Async-Worker (siehe
  2.6). Beides ist nicht Teil der Befunde, zeigt aber dieselbe Wirkung wie
  B-4 in kleinerem Maß.

---

## 4. Was sich bestätigt hat

- **Eine Verbindung je Run:** 3.900 gesendete Nachrichten aus 900
  Einzel-Sends und 3 Runs ergaben an der Senke 903 Verbindungen.
- **Der Anhang bleibt auf dem Server:** Der Draft einer 8-MB-MDM ist 516
  Bytes groß.
- **Der Import streamt** und hält den Store im Limit.
- **Bauen im Hintergrund wirkt:** Runs und Resends großer Nachrichten
  (`prepare` in `spawn_blocking`) lassen die ACK-Latenz unberührt.
- **Die Run-Grenze greift** bei unveränderten Nachrichten sofort (`413` in
  1 ms).
- **Die Oberfläche bleibt ruhig:** 250-ms-Batching und der Prepend-Pfad
  halten auch 2.000 Nachrichten/s ohne Ruckeln aus; die 1000
  `delivery_updated`-Ereignisse eines Runs verursachen keine Long Tasks.

---

## 5. Empfohlene Reihenfolge

1. **B-1** vor dem Einschalten von `send.enabled` auf einem Server mit
   MDM-Verkehr.
2. **B-2 und B-3** — sie treffen den täglichen Draft-Ablauf; B-6 liegt in
   derselben Funktion wie B-2.
3. **B-4 und B-5** — kleine Änderungen mit gemessenem Effekt; B-7 halbiert
   nebenbei die teuerste Anfrage aus B-4.
4. **B-8** fällt mit B-1 zusammen, wenn die Kopien erst beim Senden gebaut
   werden.
5. **B-9 und B-10** bei Gelegenheit.

---

## Befund-Index → Issues

| Befund | Schwere | Issue | Kernbereich |
|---|---|---|---|
| B-1 | Hoch | [#298](https://github.com/Pappet/harux/issues/298) | Senden / Speicherlimit |
| B-2 | Mittel | [#299](https://github.com/Pappet/harux/issues/299) | Frontend / Draft-Editor |
| B-3 | Mittel | [#300](https://github.com/Pappet/harux/issues/300) | Frontend / localStorage |
| B-4 | Mittel | [#301](https://github.com/Pappet/harux/issues/301) | API / Nebenläufigkeit |
| B-5 | Mittel | [#302](https://github.com/Pappet/harux/issues/302) | MLLP-Client / Runs |
| B-6 | Niedrig | [#303](https://github.com/Pappet/harux/issues/303) | Frontend / Async-Race |
| B-7 | Niedrig | [#304](https://github.com/Pappet/harux/issues/304) | Frontend / Diff-Tab |
| B-8 | Niedrig | [#305](https://github.com/Pappet/harux/issues/305) | Senden / Platzhalter |
| B-9 | Niedrig | [#306](https://github.com/Pappet/harux/issues/306) | API / Routing |
| B-10 | Niedrig | [#307](https://github.com/Pappet/harux/issues/307) | Logging |
