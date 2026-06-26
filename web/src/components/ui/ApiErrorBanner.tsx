interface ApiErrorBannerProps {
  errors: (Error | null | undefined)[]
}

export function ApiErrorBanner({ errors }: ApiErrorBannerProps) {
  const hasError = errors.some(e => e != null)

  if (!hasError) return null

  return (
    <div className="rounded-md border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
      Some data may be stale — API unavailable
    </div>
  )
}
