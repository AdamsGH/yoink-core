import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'

import { Button } from '@ui'

export default function UnauthorizedPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  return (
    <div className="flex flex-col items-center justify-center min-h-screen gap-4 p-4 text-center">
      <h1 className="text-2xl font-bold">{t('unauthorized.title')}</h1>
      <p className="text-muted-foreground">{t('unauthorized.message')}</p>
      <Button variant="link" onClick={() => navigate(-1)}>
        {t('unauthorized.back')}
      </Button>
    </div>
  )
}
