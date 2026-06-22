import { Component, type ReactNode, type ErrorInfo } from 'react'
import { AlertTriangle, RefreshCw } from 'lucide-react'
import { Button } from './ui/Button'
import { Card, CardContent } from './ui/Card'

interface Props {
  children: ReactNode
  fallback?: ReactNode
}

interface State {
  hasError: boolean
  error: Error | null
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props)
    this.state = { hasError: false, error: null }
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error }
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo): void {
    console.error('[ErrorBoundary] Caught error:', error, errorInfo)
  }

  handleRetry = () => {
    this.setState({ hasError: false, error: null })
    window.location.reload()
  }

  render() {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback
      }

      return (
        <div className="flex items-center justify-center min-h-[400px] p-6">
          <Card className="max-w-md w-full">
            <CardContent className="p-8 text-center">
              <AlertTriangle className="w-12 h-12 text-red-500 mx-auto mb-4" />
              <h2 className="text-xl font-semibold text-text mb-2">Something went wrong</h2>
              <p className="text-text-muted mb-6 text-sm">
                {this.state.error?.message || 'An unexpected error occurred'}
              </p>
              <Button onClick={this.handleRetry} variant="primary">
                <RefreshCw className="w-4 h-4 mr-2" />
                Reload
              </Button>
            </CardContent>
          </Card>
        </div>
      )
    }

    return this.props.children
  }
}
