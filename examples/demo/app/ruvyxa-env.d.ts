declare module "*.css" {
  const content: Record<string, string>
  export default content
}

interface ImportMetaEnv {
  RUVYXA_PUBLIC_APP_NAME: string
  RUVYXA_PUBLIC_API_URL: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
