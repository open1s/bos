import React from 'react'
import { useTodos } from '../hooks/useTodos'
import { Todo } from '../hooks/useTodos'

function TodoItem({ t }: { t: Todo }) {
  return (
    <li>
      {t.title} {t.completed ? '✓' : ''}
    </li>
  )
}

export function TodosList() {
  const { data, isLoading, isError } = useTodos()
  if (isLoading) return <div>Loading...</div>
  if (isError) return <div>Error loading todos</div>
  return (
    <ul>
      {data?.map((t) => (
        <TodoItem key={t.id} t={t} />
      ))}
    </ul>
  )
}
