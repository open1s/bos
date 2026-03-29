import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../api/apiClient'
export interface Todo {
  id: string
  title: string
  completed: boolean
}

export function useTodos() {
  return useQuery<Todo[], Error>({
    queryKey: ['todos'],
    queryFn: () => apiGet<Todo[]>('/api/todos'),
  })
}
