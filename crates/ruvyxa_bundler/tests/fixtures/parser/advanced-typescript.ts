interface Config {
  port: number
}

type Resource = { close(): void }

const enum Mode {
  Development,
  Production = 5,
}

function sealed(value: unknown) {
  return value
}

@sealed
export class Service implements Disposable {
  readonly config: Config = { port: 3000 } satisfies Config

  [Symbol.dispose]() {}
}

declare function openResource(): Resource

export async function loadResource() {
  await using resource = openResource()
  return resource as Resource
}

export { Mode }
