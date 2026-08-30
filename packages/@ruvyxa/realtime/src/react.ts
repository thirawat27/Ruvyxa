/**
 * React bindings for Ruvyxa's native collaboration rooms.
 *
 * One provider owns one socket. Hooks read from it through
 * `useSyncExternalStore`, so a room with many subscribed components still holds
 * a single connection and every component sees the same snapshot in the same
 * render pass.
 *
 * These bindings live here rather than in `@ruvyxa/react` so the dependency
 * points one way: this package knows about React, and React apps that never
 * open a room never pull the transport in.
 */

import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useSyncExternalStore,
} from 'react'
import type { ReactNode } from 'react'

import { createCollabClient } from './collab.js'
import type { CollabClient, CollabClientOptions, CollabPeer, CollabSnapshot } from './collab.js'

export type {
  CollabClient,
  CollabClientOptions,
  CollabEntry,
  CollabPeer,
  CollabSnapshot,
} from './collab.js'

const CollabContext = createContext<CollabClient | undefined>(undefined)

export interface CollabProviderProps extends CollabClientOptions {
  children?: ReactNode
}

/**
 * Open one collaboration room for the tree below it.
 *
 * The client is recreated when the room changes and closed on unmount, so
 * navigating between documents never leaves an orphaned socket behind.
 */
export function CollabProvider({ children, ...options }: CollabProviderProps) {
  const client = useMemo(
    () => createCollabClient(options),
    // Transport options are read once when the socket opens; only the room
    // identity should force a reconnect, so the rest are deliberately not
    // dependencies. Changing them mid-session requires remounting the provider.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [options.room, options.url],
  )
  useEffect(() => () => client.close(), [client])
  return createElement(CollabContext.Provider, { value: client }, children)
}

/** The room client for the nearest `CollabProvider`. */
export function useCollabClient(): CollabClient {
  const client = useContext(CollabContext)
  if (!client) {
    throw new Error('useCollabClient must be called inside a <CollabProvider>')
  }
  return client
}

/** The full room snapshot, re-rendering whenever any part of it changes. */
export function useCollabRoom(): CollabSnapshot {
  const client = useCollabClient()
  return useSyncExternalStore(
    useCallback((onChange) => client.subscribe(onChange), [client]),
    client.snapshot,
    client.snapshot,
  )
}

/**
 * Publish this client's presence and read everyone else's.
 *
 * Pass the local state to publish — a cursor position, a selection, a name. The
 * returned peers exclude the local one, because a component that draws remote
 * cursors almost never wants to draw its own on top of the real pointer.
 */
export function usePresence<T>(state: T): readonly CollabPeer[] {
  const client = useCollabClient()
  const room = useCollabRoom()
  // `state` is already an effect dependency, so the effect always runs with the
  // newest value and reads it directly. A ref used to hold it, written during
  // render — which is unsafe under concurrent rendering, and bought nothing:
  // the ref could not stabilise a dependency that was in the list beside it.
  useEffect(() => {
    client.setPresence(state)
  }, [client, state])
  return useMemo(() => room.peers.filter((peer) => !peer.self), [room.peers])
}

/**
 * Read and write one shared-state key, last-writer-wins.
 *
 * The returned value is whatever the server last accepted for this key, so two
 * peers writing at once converge instead of diverging — but the later write
 * replaces the earlier one rather than merging with it. Split a document across
 * keys when concurrent edits must both survive.
 *
 * The setter returns `false` when the socket was not open: the write is held
 * for the next reconnect rather than sent, and until then the value read here
 * is still the server's, because this hook renders server state only. React
 * ignores the return of an event handler, so an `onChange` may keep discarding
 * it — but a component that wants to show "not saved yet" can read it, which is
 * the only signal available while the room is down.
 */
export function useSharedState<T>(
  key: string,
  fallback: T,
): readonly [T, (value: T | null) => boolean] {
  const client = useCollabClient()
  const room = useCollabRoom()
  const entry = room.state[key]
  const value = entry === undefined ? fallback : (entry.value as T)
  const setValue = useCallback((next: T | null) => client.setState({ [key]: next }), [client, key])
  return [value, setValue]
}
