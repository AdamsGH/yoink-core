export default function ApiDocsPage() {
  return (
    <div className="-mx-4 -my-6 h-[calc(100vh-3rem)] md:h-[calc(100svh-3rem)]">
      <iframe
        src="/scalar/"
        className="h-full w-full border-none"
        title="API Docs"
        allow="clipboard-write"
      />
    </div>
  )
}
