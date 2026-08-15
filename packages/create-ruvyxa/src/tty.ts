/**
 * In-place terminal redrawing for the scaffolder's banner and menu.
 *
 *
 * Both used to reposition with `\x1b[s` / `\x1b[u` — DECSC and DECRC, save and
 * restore cursor. Those store an *absolute* screen position. The banner is ten
 * lines tall, so drawing it with the cursor near the bottom of the viewport
 * scrolls the terminal, and scrolling moves the content without moving the
 * saved position: the mark now names a screen row holding something else.
 * Every subsequent restore landed below the banner it was supposed to overwrite
 * and `\x1b[J` cleared from there, so each frame appended a fresh copy instead
 * of replacing the previous one. On a short terminal that produced a column of
 * mascots, one per animation frame, four times a second.
 *
 * Relative movement has no such failure mode. `\x1b[<n>A` moves up `n` rows
 * from wherever the cursor actually is, which is still correct after the
 * terminal has scrolled, so this module counts the rows it wrote and walks back
 * over exactly that many.
 */

/** Assumed terminal size when the stream does not report one. */
const DEFAULT_COLUMNS = 80
const DEFAULT_ROWS = 24

/**
 * Characters a terminal shows for `line`, ignoring styling.
 *
 * Colour is written as CSI sequences that occupy no columns. Counting them
 * would overstate the width and make a line look wrapped when it is not.
 */
export function visibleWidth(line: string): number {
  return stripAnsi(line).length
}

/** Remove CSI sequences, leaving only what the terminal displays. */
export function stripAnsi(value: string): string {
  let visible = ''
  let index = 0
  while (index < value.length) {
    if (value[index] !== '\x1b') {
      visible += value[index]
      index += 1
      continue
    }
    index += 1
    if (value[index] !== '[') continue
    index += 1
    // Parameter and intermediate bytes run until a final byte in 0x40..=0x7E.
    while (index < value.length) {
      const code = value.charCodeAt(index)
      index += 1
      if (code >= 0x40 && code <= 0x7e) break
    }
  }
  return visible
}

/**
 * Screen rows `lines` occupies once the terminal has wrapped them.
 *
 * A logical line wider than the viewport becomes several physical rows, and
 * walking back by the logical count would leave the overflow on screen. An
 * empty line still occupies one row.
 */
export function physicalRows(lines: readonly string[], columns: number): number {
  const width = columns > 0 ? columns : DEFAULT_COLUMNS
  let rows = 0
  for (const line of lines) {
    const visible = visibleWidth(line)
    rows += visible === 0 ? 1 : Math.ceil(visible / width)
  }
  return rows
}

/**
 * The escape sequence that returns the cursor to the start of a frame that
 * occupied `rows` screen rows, and clears it.
 *
 * The cursor sits on the last row of the frame, because frames are written
 * without a trailing newline — so it walks up `rows - 1`, not `rows`. `\r`
 * returns to column zero and `\x1b[0J` erases from there to the end of the
 * screen.
 */
export function rewindSequence(rows: number): string {
  if (rows <= 0) return ''
  const up = rows > 1 ? `\x1b[${rows - 1}A` : ''
  return `${up}\r\x1b[0J`
}

/** Minimal view of the parts of `process.stdout` this module writes to. */
export interface FrameStream {
  write(chunk: string): unknown
  columns?: number
  rows?: number
}

/**
 * A region of the terminal that can be redrawn in place.
 *
 * `render` replaces whatever the previous `render` wrote. `finish` leaves the
 * last frame on screen and moves past it, so ordinary output continues below.
 */
/** @public — the declared return type of `createFrame`, so callers can name it. */
export interface Frame {
  render(lines: readonly string[]): void
  /** Draw one last frame (or keep the current one) and release the region. */
  finish(lines?: readonly string[]): void
  /**
   * Erase the region, leaving the cursor where the frame began.
   *
   * Used by a prompt whose answer is echoed elsewhere: nothing it drew should
   * remain once it is done.
   */
  clear(): void
  /**
   * Whether the region can actually be redrawn.
   *
   * `false` when the frame is at least as tall as the viewport: the top would
   * have scrolled out of reach, and walking back over rows that no longer exist
   * corrupts output above. Callers use this to fall back to drawing once
   * instead of animating.
   */
  canRedraw(lines: readonly string[]): boolean
}

/**
 * Create a redrawable region on `stream`.
 *
 * `hideCursor` is off by default so a non-animating caller does not have to
 * remember to restore it.
 */
export function createFrame(stream: FrameStream, hideCursor = false): Frame {
  let previousRows = 0
  let hidden = false

  const columns = () => stream.columns ?? DEFAULT_COLUMNS
  const viewportRows = () => stream.rows ?? DEFAULT_ROWS

  const canRedraw = (lines: readonly string[]) => physicalRows(lines, columns()) < viewportRows()

  const write = (lines: readonly string[]) => {
    stream.write(lines.join('\n'))
    previousRows = physicalRows(lines, columns())
  }

  const showCursor = () => {
    if (!hidden) return
    stream.write('\x1b[?25h')
    hidden = false
  }

  return {
    canRedraw,
    render(lines) {
      if (hideCursor && !hidden) {
        stream.write('\x1b[?25l')
        hidden = true
      }
      if (previousRows > 0) stream.write(rewindSequence(previousRows))
      write(lines)
    },
    finish(lines) {
      if (lines) this.render(lines)
      stream.write('\n')
      previousRows = 0
      showCursor()
    },
    clear() {
      if (previousRows > 0) stream.write(rewindSequence(previousRows))
      previousRows = 0
      showCursor()
    },
  }
}
