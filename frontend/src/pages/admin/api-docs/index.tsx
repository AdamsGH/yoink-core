export default function ApiDocsPage() {
  return (
    <div className="-mx-4 -my-6" style={{ height: 'calc(100svh - 3rem)' }}>
      <iframe
        src="/docs"
        className="h-full w-full border-none"
        title="API Docs"
        allow="clipboard-write"
      />
    </div>
  )
}
