/**
 * Deterministic cache-budget policy shared by worker cache owners.
 *
 * The controller owns policy and counters, not cached values. Callers evict
 * their own entries in the returned order, which keeps ownership explicit and
 * prevents this module from reaching across cache implementation boundaries.
 */
export class CachePressureController {
  #hardLimitBytes
  #softLimitBytes
  #targetBytes
  #pressureLevel = 'none'
  #pressureEvents = 0
  #evictions = new Map()

  constructor({ hardLimitBytes, softRatio = 0.8, targetRatio = 0.65 }) {
    if (!Number.isSafeInteger(hardLimitBytes) || hardLimitBytes <= 0) {
      throw new TypeError('hardLimitBytes must be a positive safe integer')
    }
    if (!(targetRatio > 0 && targetRatio < softRatio && softRatio < 1)) {
      throw new RangeError('cache pressure ratios must satisfy 0 < target < soft < 1')
    }
    this.#hardLimitBytes = hardLimitBytes
    this.#softLimitBytes = Math.floor(hardLimitBytes * softRatio)
    this.#targetBytes = Math.floor(hardLimitBytes * targetRatio)
  }

  observe(residentBytes) {
    if (!Number.isFinite(residentBytes) || residentBytes < 0) {
      throw new TypeError('residentBytes must be a non-negative finite number')
    }
    const previous = this.#pressureLevel
    if (residentBytes >= this.#hardLimitBytes) {
      this.#pressureLevel = 'hard'
    } else if (residentBytes >= this.#softLimitBytes) {
      this.#pressureLevel = 'soft'
    } else if (residentBytes <= this.#targetBytes) {
      this.#pressureLevel = 'none'
    }
    if (this.#pressureLevel !== 'none' && this.#pressureLevel !== previous) {
      this.#pressureEvents++
    }
    return {
      level: this.#pressureLevel,
      targetBytes: this.#targetBytes,
      toFreeBytes: Math.max(0, Math.ceil(residentBytes - this.#targetBytes)),
      stopSpeculation: this.#pressureLevel === 'hard',
    }
  }

  recordEviction(kind, entries = 1) {
    if (!Number.isSafeInteger(entries) || entries < 0) {
      throw new TypeError('evicted entries must be a non-negative safe integer')
    }
    this.#evictions.set(kind, (this.#evictions.get(kind) ?? 0) + entries)
  }

  snapshot(residentBytes) {
    return {
      hardLimitBytes: this.#hardLimitBytes,
      softLimitBytes: this.#softLimitBytes,
      targetBytes: this.#targetBytes,
      residentBytes,
      pressureLevel: this.#pressureLevel,
      pressureEvents: this.#pressureEvents,
      evictions: Object.fromEntries(
        [...this.#evictions].sort(([left], [right]) => left.localeCompare(right)),
      ),
    }
  }
}

/** Entry-count-bounded LRU with explicit exclusion for in-flight owners. */
export class LruCache {
  #max
  #map = new Map()

  constructor(max) {
    if (!Number.isSafeInteger(max) || max <= 0) throw new TypeError('LRU max must be positive')
    this.#max = max
  }

  get(key) {
    if (!this.#map.has(key)) return undefined
    const value = this.#map.get(key)
    this.#map.delete(key)
    this.#map.set(key, value)
    return value
  }

  set(key, value) {
    let evicted
    if (this.#map.has(key)) {
      this.#map.delete(key)
    } else if (this.#map.size >= this.#max) {
      const evictedKey = this.#map.keys().next().value
      evicted = { key: evictedKey, value: this.#map.get(evictedKey) }
      this.#map.delete(evictedKey)
    }
    this.#map.set(key, value)
    return evicted
  }

  has(key) {
    return this.#map.has(key)
  }

  delete(key) {
    const value = this.#map.get(key)
    this.#map.delete(key)
    return value
  }

  clear() {
    const size = this.#map.size
    this.#map.clear()
    return size
  }

  get size() {
    return this.#map.size
  }

  keys() {
    return this.#map.keys()
  }

  evictOldest(excluded = new Set()) {
    for (const key of this.#map.keys()) {
      if (excluded.has(key)) continue
      const value = this.#map.get(key)
      this.#map.delete(key)
      return { key, value }
    }
    return undefined
  }
}
