import { useParams } from "react-router"

/** A path param that the matched route always defines. */
export function useRequiredParam(name: string): string {
  const value = useParams()[name]
  if (value === undefined) {
    throw new Error(`Route is missing the :${name} param`)
  }
  return value
}
