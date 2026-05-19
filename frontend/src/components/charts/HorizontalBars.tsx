export function HorizontalBars({ data, nameKey, valueKey, colors }: {
  data: Array<Record<string, unknown>>
  nameKey: string
  valueKey: string
  colors: string[]
}) {
  if (!data.length) return null
  const max = Math.max(...data.map(d => Number(d[valueKey])))
  return (
    <div className="space-y-1.5">
      {data.map((d, i) => {
        const pct = Math.round((Number(d[valueKey]) / max) * 100)
        return (
          <div key={String(d[nameKey])} className="flex items-center gap-2 text-xs">
            <span className="text-muted-foreground w-4 tabular-nums shrink-0">{i + 1}</span>
            <div className="flex-1 min-w-0">
              <div className="flex justify-between mb-0.5">
                <span className="truncate">{String(d[nameKey])}</span>
                <span className="tabular-nums font-medium ml-2 shrink-0">{Number(d[valueKey])}</span>
              </div>
              <div className="h-1.5 rounded-full bg-muted-foreground/20">
                <div
                  className="h-1.5 rounded-full transition-all"
                  style={{ width: `${pct}%`, backgroundColor: colors[i % colors.length] }}
                />
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
}
