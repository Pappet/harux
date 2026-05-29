# Harux — Architektur- & Logik-Audit (2026-05-29)

> Ganzheitliches Code-Audit der Codebasis (Backend `src/`, Frontend `static/`,
> Konfiguration, Tests, Assets). Fokus auf Makroebene, strukturelle Integrität
> und datei-/modulübergreifende Logik-Risse — bewusst **nicht** auf triviale
> Linter-/Formatierungsfragen.
>
> Alle Findings sind als GitHub-Issues erfasst (#177–#192) und unten verlinkt.

## Inhaltsverzeichnis

- [Kontext-Einordnung](#kontext-einordnung)
- [1. Kritische Bedrohungen (Top 5)](#1-kritische-bedrohungen-top-5)
- [2. Detaillierte Schwachstellenanalyse](#2-detaillierte-schwachstellenanalyse)
- [3. Refactoring-Roadmap](#3-refactoring-roadmap)
- [Finding-Index → Issues](#finding-index--issues)

---

## Kontext-Einordnung

Die Codebasis ist handwerklich **überdurchschnittlich** für KI-gestützte,
iterative Generierung: konsistente Modulgrenzen, dokumentierte Invarianten,
brauchbare Tests, durchdachte Kommentare. Es gibt **keine** zirkulären
Abhängigkeiten und kaum Spaghetti-Code.

Die gefährlichen Risse liegen daher nicht an der Oberfläche, sondern in
**datei- und prozessübergreifenden Annahmen**, die im Laufe der Iterationen
auseinandergedriftet sind — der typische Effekt von Kontextverlust über viele
Generierungsschritte hinweg.

**Verifizierte Kopplungen:**

- Die MSH-Index-Verschiebung in `parser.rs` deckt sich exakt mit der
  Dictionary-Nummerierung (`v2.5.1.json`: seq 1 = Field Separator) — die
  Kopplung ist funktionierend, aber implizit und ungetestet (→ S-8).
- Die „prepend"-Logik ist nachweislich ein Widerspruch zu einer dokumentierten
  Invariante (→ KB-5).

---

## 1. Kritische Bedrohungen (Top 5)

### KB-1 — Das Frontend hat keine Eviction: `state.messages` wächst unbegrenzt

[Issue #177](https://github.com/Pappet/harux/issues/177)

Der Server ist sorgfältig auf 10.000 Nachrichten / 512 MB gedeckelt
(`src/store.rs`). Der Client kennt diese Grenze **nicht**: `addMessage`
(`static/ws.js:129`) macht `state.pendingMessages.unshift(...)`,
`flushAndRender` (`static/ws.js:161`) hängt alles dauerhaft an
`state.messages` — **nichts trimmt dieses Array jemals**.

Für das dokumentierte Deployment („mehrere Entwickler gleichzeitig per Browser,
kein lokales Setup, dauerlaufende Tabs") ist das die mit Abstand größte Gefahr:
Ein Tab, der über Stunden an einem Orchestra-Feed hängt, akkumuliert
Hunderttausende Zeilen im DOM (`prependMessagesToList` fügt unbegrenzt
`.message-row`-Knoten ein) → OOM des Browsers. Der Server wirkt robust, das
System fällt am Client.

### KB-2 — Der 512-MB-Speicherdeckel ist umgehbar (Server-OOM)

[Issue #178](https://github.com/Pappet/harux/issues/178)

Drei sich überlagernde Lücken in `src/store.rs::run_eviction_loop`:

- **Bookmark-Starvation:** Sind die ältesten Nachrichten alle gebookmarkt,
  werden sie wieder nach vorn geschoben, `evicted == 0`, es wird nur geloggt
  (`src/store.rs:322-337`). `current_bytes` bleibt über dem Limit, das Flag
  wird zurückgesetzt, die Notify konsumiert. Ein Nutzer, der genügend große
  MDM-Nachrichten (mehrere MB Base64) bookmarkt, **hebt den Speicherdeckel
  vollständig auf** — bei Multi-User ein realer DoS-Vektor.
- **`target_count == 0` → `continue`** (`src/store.rs:290-293`): Bei niedrigem
  Byte-Limit und wenigen großen Nachrichten wird nie evakuiert, bis ≥10
  Nachrichten vorliegen → der Store überschreitet den konfigurierten Speicher
  um bis zu ~10×. Der eigene Test `test_byte_based_eviction_triggers` passt nur
  knapp (9 < 10) und maskiert genau diesen Defekt.
- **Eviction zielt auf Anzahl, nicht auf Bytes:** Es werden „10 % der
  Nachrichten" entfernt, nicht „so viel, bis `current_bytes < max_bytes`".
  Eine Eviction-Runde kann den Deckel verfehlen; eine erneute Auslösung
  passiert erst beim nächsten Insert. Während eines Bursts steht der Store
  dauerhaft über dem Limit.

### KB-3 — Byte-Buchhaltung driftet systematisch nach unten

[Issue #179](https://github.com/Pappet/harux/issues/179)

`insert` addiert `estimated_bytes()` **vor** der `Arc`-Wrapping
(`src/store.rs:101-103`). Spätere Mutationen über `Arc::make_mut` (Tags
hinzufügen, `add_tag`/`toggle_bookmark`) vergrößern den realen Heap,
**aktualisieren `current_bytes` aber nicht**. Bei der Eviction wird dann
`removed.estimated_bytes()` (Post-Mutation, also größer) abgezogen
(`src/store.rs:312, 339`).

Folge: `current_bytes` driftet über die Lebenszeit nach unten → der
byte-basierte Trigger feuert immer seltener → der reale Speicher überschreitet
still das Limit. Verschärft KB-2.

### KB-4 — Synchrones Parsen/Validieren im MLLP-Lesepfad + 2–3× String-Duplikat-Footprint

[Issue #180](https://github.com/Pappet/harux/issues/180)

`parse_message` (`src/hl7/parser.rs:5-15`) führt fünf Pässe **synchron** aus,
bevor `store.insert` aufgerufen wird — die Verbindung kann den nächsten Frame
erst danach lesen. Schwerwiegender ist der Speicher-Multiplikator: Pro Feld
werden gehalten:

- `value` (String, `src/hl7/parser.rs:213`)
- `components` — jede Komponente erneut als eigener `String`
  (`src/hl7/parser.rs:207-210`)
- `description` — geklonter Dictionary-String pro Feld
  (`src/dictionary.rs:71-74`)

Eine 5-MB-Base64-OBX-Zeile liegt damit als `value` (5 MB) **und**
`components[0]` (5 MB) **und** im `raw` → ~3× `raw`. `estimated_bytes` zählt das
korrekt mit, aber in Verbindung mit KB-2/KB-3 bedeutet es, dass der nominelle
512-MB-Deckel real deutlich früher gegen physischen RAM läuft. Bei
MDM-Lastprofil (laut `CLAUDE.md` das Zielprofil) ist das der wahrscheinlichste
Produktions-Crash.

### KB-5 — Dokumentierte Architektur-Invariante wurde durch spätere Iteration aufgebrochen

[Issue #181](https://github.com/Pappet/harux/issues/181)

`CLAUDE.md:130` (Constraints-Tabelle) sagt **explizit**: „DOM rendering | No
prepend logic (batching at 250ms is sufficient)". Trotzdem existieren
`canPrependOnly()` (`static/render.js:307`) und `prependMessagesToList()`
(`static/render.js:314`) und sind laut `CHANGELOG.md` der bewusst eingeführte
Hot-Path in `flushAndRender`.

Das ist das klassische KI-Kontextverlust-Artefakt: eine als „entschieden"
markierte Designregel wurde von einem späteren Performance-Commit überschrieben,
**ohne die Invariante zu aktualisieren**. Die Single Source of Truth über die
Architektur ist jetzt gespalten, und der nächste Agent arbeitet auf Basis einer
falschen Regel.

---

## 2. Detaillierte Schwachstellenanalyse

### Datenfluss & State-Management

**S-1 · Gespaltene Quelle der Wahrheit für „total"**
([#182](https://github.com/Pappet/harux/issues/182), `static/ws.js:220`,
`static/render.js:179-183`)
`pollStats` schreibt alle 3 s die autoritative Server-Zählung in
`state.totalMessagesCount` — aber `updateHeaderCounters` zeigt
`state.messages.length + state.pendingMessages.length` an. Nach serverseitiger
Eviction divergieren beide dauerhaft. `totalMessagesCount` wird damit faktisch
write-only / verwaister State. Gleiches gilt für `totalValidationCount` /
`sourceCounts`: hochgezählt, bei Eviction nie dekrementiert → angezeigte Zähler
werden über die Zeit falsch.

**S-2 · Async-Race bei Detailauswahl**
([#183](https://github.com/Pappet/harux/issues/183), `static/render.js:496-510`,
`static/diff.js:154-174`)
`selectMessage` ist `async` und `await`et `fetch`. Klickt der Nutzer schnell
A→B, kann A's Antwort nach B's eintreffen und `state.selectedMessage` mit A
überschreiben, während Zeile B markiert ist → Detailpanel zeigt die falsche
Nachricht. Keine Request-Sequenzierung/Abort. Identisch in `toggleDiffPin`.

**S-3 · Doppelzählung in `updateMessageBookmark`**
([#184](https://github.com/Pappet/harux/issues/184), `static/ws.js:93-126`)
`totalBookmarkCount` wird unabhängig für `listMsg` und `pendingMsg` angepasst.
Ist eine Nachricht gleichzeitig in beiden Listen referenziert, wird ±1 doppelt
gebucht.

### HL7-Logik & Korrektheit

**S-4 · Repetition/Sub-Komponente/Escape werden nie verarbeitet**
([#185](https://github.com/Pappet/harux/issues/185), `src/hl7/types.rs:99-105`,
`src/hl7/parser.rs:206-218`)
`Delimiters.repetition/.escape/.subcomponent` sind `#[allow(dead_code)]`. Der
Parser splittet nur auf `component` (`^`). Wiederholungsfelder werden nicht
aufgetrennt; Sub-Komponenten und HL7-Escapes werden roh angezeigt → fachlich
falsche Darstellung in einem „Message-Analysis"-Tool. Die Datenstruktur
verspricht 5 Delimiter, die Logik liefert 1.

**S-5 · `message_structure` (MSH-9.3) befüllt, aber nie konsumiert**
([#186](https://github.com/Pappet/harux/issues/186), `src/hl7/parser.rs:60-64`,
`src/validation.rs:46`)
Der Kommentar verspricht Nutzung in Milestone-3-Validierung; tatsächlich matcht
`validate_message` nur auf `msg.message_type`. Spekulativ vorab eingeführtes,
totes Datenfeld.

**S-6 · Inkonsistenz ACK vs. NACK**
([#187](https://github.com/Pappet/harux/issues/187), `src/hl7/parser.rs:258-281`)
`build_ack` und `build_nack` erzeugen MSH-9 / Control-ID / MSA unterschiedlich.
Zwei divergente Implementierungen desselben Konzepts.

**S-7 · Doppelte, abweichende MSH-Parselogik + Doppel-Parse**
([#188](https://github.com/Pappet/harux/issues/188), `src/mllp.rs:301-324`,
`src/mllp.rs:276-292`)
`extract_msh18` re-implementiert MSH-Splitting getrennt vom Haupt-Parser. Bei
ungültigem MSH-2 extrahiert es den Charset trotzdem, während `parse_delimiters`
ablehnt. Zudem dekodiert `extract_mllp_frame` die Bytes via MSH-18 in einen
String, den `parse_message` erneut parst → Mojibake bei falsch deklariertem
Charset, ohne Fallback-Validierung.

### Robustheit & Fehlerbehandlung

**S-8 · Ungeprüfte Cross-File-Kopplung MSH-Shift ↔ Dictionary**
([#189](https://github.com/Pappet/harux/issues/189), `src/hl7/parser.rs:223-236`)
Die synthetische `MSH-1`-Einfügung + `+1`-Shift funktioniert nur, weil
`v2.5.1.json` MSH mit `seq 1 = Field Separator` nummeriert (verifiziert). Nicht
durch Test abgesichert. Ändert sich das Dictionary-Schema (vgl. #49), verschiebt
sich jede MSH-Datentyp-Prüfung und -Beschreibung still um 1.

**S-9 · Verschluckte Fehler an mehreren Rändern**
([#190](https://github.com/Pappet/harux/issues/190), `static/util.js:84,109`,
`static/ws.js:205`, `src/mllp.rs:236`)
Mehrere `let _ = ...` / leere `catch`. Insbesondere der NACK-Write wird ohne
Fehlerbehandlung verworfen — schlägt er fehl, erfährt es niemand.

**S-10 · Zeit-/Sleep-basierte Eviction-Tests sind flaky**
([#191](https://github.com/Pappet/harux/issues/191), `src/store.rs:431,456,501`)
Die Eviction ist event-getrieben (Notify), die Tests synchronisieren über
`tokio::time::sleep` → Race-anfällig unter CI-Last.

**S-11 · `Arc::make_mut` kann Multi-MB-Deepclone unter dem Write-Lock auslösen**
([#192](https://github.com/Pappet/harux/issues/192), `src/store.rs:202`)
Hält ein Reader gerade einen `Arc`-Snapshot einer großen MDM-Nachricht, klont
ein gleichzeitiges `toggle_bookmark`/`add_tag` die komplette Nachricht unter
gehaltenem `RwLock`-Write → Latenz-/Speicher-Spike, der alle Schreiber
blockiert.

---

## 3. Refactoring-Roadmap

### Phase 0 — Invarianten reparieren (Stunden, kein Risiko)

1. `CLAUDE.md:130` mit der Realität abgleichen (prepend ist gewollt → Constraint
   umformulieren) **oder** prepend entfernen. KB-5 auflösen, bevor weiter gebaut
   wird.
2. Verwaisten/spekulativen State markieren oder entfernen: `totalMessagesCount`
   als Quelle der Wahrheit klären (S-1), `message_structure` (S-5).
3. Regressionstest für die MSH-Shift↔Dictionary-Kopplung hinzufügen (S-8).

### Phase 1 — Speicher-Integrität (das Produktions-Crash-Cluster)

4. KB-2: Eviction byte-zielgerichtet machen (Schleife bis
   `current_bytes < max_bytes`), `target_count==0`-Pfad korrigieren,
   Bookmark-Speicher hart deckeln.
5. KB-3: `current_bytes` bei jeder `mutate_message`-Mutation als Delta
   nachführen (alt vs. neu `estimated_bytes`).
6. KB-1: Client-seitige Kappung von `state.messages` (Ringpuffer passend zur
   Server-Kapazität) + entsprechendes DOM-Trimmen im prepend-Pfad.

### Phase 2 — Hot-Path & Nebenläufigkeit

7. KB-4: Dictionary-Beschreibungen nicht pro Feld klonen — `&'static str` /
   Index referenzieren; `components` lazy / `Cow`. Validierung/Injektion ggf.
   aus dem Lesepfad in einen separaten Task verlagern.
8. S-2: Request-Sequenzierung in `selectMessage`/`toggleDiffPin`
   (`AbortController`, Abgleich `resp.id === state.selectedId`).
9. S-11: Veränderliche Felder (`tags`/`bookmarked`) aus dem großen
   `Arc<Hl7Message>` herauslösen, sodass Mutationen keinen Deepclone erfordern.

### Phase 3 — HL7-Korrektheit (zahlt aufs aktuelle Milestone ein)

10. S-4: Repetition/Sub-Komponente/Escape im Parser umsetzen.
11. S-7/S-6: MSH-Parsing vereinheitlichen; ACK/NACK auf einen Builder
    konsolidieren.
12. S-10: Eviction-Tests deterministisch machen (Test-Hook „eviction
    completed").

---

## Finding-Index → Issues

| Finding | Schweregrad | Issue | Kernbereich |
|---|---|---|---|
| KB-1 | Kritisch | [#177](https://github.com/Pappet/harux/issues/177) | Frontend-State / Memory |
| KB-2 | Kritisch | [#178](https://github.com/Pappet/harux/issues/178) | Store / Eviction |
| KB-3 | Kritisch | [#179](https://github.com/Pappet/harux/issues/179) | Store / Byte-Bilanz |
| KB-4 | Kritisch | [#180](https://github.com/Pappet/harux/issues/180) | Parser / Memory / Hot-Path |
| KB-5 | Kritisch | [#181](https://github.com/Pappet/harux/issues/181) | Architektur-Invariante |
| S-1 | Mittel | [#182](https://github.com/Pappet/harux/issues/182) | Frontend-State |
| S-2 | Mittel | [#183](https://github.com/Pappet/harux/issues/183) | Frontend / Async-Race |
| S-3 | Niedrig | [#184](https://github.com/Pappet/harux/issues/184) | Frontend-State |
| S-4 | Mittel | [#185](https://github.com/Pappet/harux/issues/185) | HL7-Parser |
| S-5 | Niedrig | [#186](https://github.com/Pappet/harux/issues/186) | Toter Code |
| S-6 | Niedrig | [#187](https://github.com/Pappet/harux/issues/187) | HL7 ACK/NACK |
| S-7 | Mittel | [#188](https://github.com/Pappet/harux/issues/188) | MLLP / Charset |
| S-8 | Mittel | [#189](https://github.com/Pappet/harux/issues/189) | Parser ↔ Dictionary |
| S-9 | Niedrig–Mittel | [#190](https://github.com/Pappet/harux/issues/190) | Fehlerbehandlung |
| S-10 | Niedrig–Mittel | [#191](https://github.com/Pappet/harux/issues/191) | Test-Stabilität |
| S-11 | Mittel | [#192](https://github.com/Pappet/harux/issues/192) | Store / Nebenläufigkeit |

---

## Gesamteinschätzung

Die **strukturelle** Integrität (Modulgrenzen, Separation of Concerns) ist gut.
Die echten Gefahren sind **logische Risse entlang von Prozessgrenzen**: Der
Speicher-Vertrag zwischen Store, Eviction und Byte-Zähler ist an drei Stellen
inkonsistent (KB-2/3/4), und der Zustands-Vertrag zwischen Server und
Browser-Client existiert faktisch nicht (KB-1, S-1). Beides schlägt nicht im
Unit-Test zu, sondern erst **unter Dauerlast im beschriebenen
Multi-User-Produktionsprofil**.
