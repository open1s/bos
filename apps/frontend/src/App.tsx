import React from 'react'
import { TodosList } from './components/TodosList'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from './queryClient'

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <div>
        <h1>App</h1>
        <TodosList />
      </div>
    </QueryClientProvider>
  )
}
