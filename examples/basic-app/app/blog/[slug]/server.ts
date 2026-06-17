import { loader } from "ruvyxa/server"

export const getPost = loader(async ({ params }) => {
  return {
    slug: params.slug,
    title: "Hello Ruvyxa",
  }
})
