import { createBrowserRouter, Navigate } from 'react-router';
import { App } from './App';
import {
  LazyRoute,
  LazyProjectsRoute,
  LazyDiscussionsRoute,
  LazyPlanningRoute,
  LazyPluginsRoute,
  LazyWorkflowsRoute,
  LazySettingsRoute,
} from './routes/lazyRoutes';

export const router = createBrowserRouter([
  {
    path: '/',
    Component: App,
    children: [
      { index: true, element: <Navigate to="/projects" replace /> },
      { path: 'projects', children: [
        { index: true, element: <LazyRoute Component={LazyProjectsRoute} /> },
        { path: ':projectId', element: <LazyRoute Component={LazyProjectsRoute} /> },
      ]},
      { path: 'discussions', children: [
        { index: true, element: <LazyRoute Component={LazyDiscussionsRoute} /> },
        { path: ':discussionId', element: <LazyRoute Component={LazyDiscussionsRoute} /> },
      ]},
      { path: 'planning', children: [
        { index: true, element: <LazyRoute Component={LazyPlanningRoute} /> },
        { path: ':taskId', element: <LazyRoute Component={LazyPlanningRoute} /> },
      ]},
      { path: 'plugins', children: [
        { index: true, element: <LazyRoute Component={LazyPluginsRoute} /> },
        { path: ':configId', element: <LazyRoute Component={LazyPluginsRoute} /> },
      ]},
      { path: 'workflows', children: [
        { index: true, element: <LazyRoute Component={LazyWorkflowsRoute} /> },
        { path: 'qp', children: [
          { index: true, element: <LazyRoute Component={LazyWorkflowsRoute} /> },
          { path: ':qpId', element: <LazyRoute Component={LazyWorkflowsRoute} /> },
        ]},
        { path: 'qa', children: [
          { index: true, element: <LazyRoute Component={LazyWorkflowsRoute} /> },
          { path: ':qaId', element: <LazyRoute Component={LazyWorkflowsRoute} /> },
        ]},
        { path: ':workflowId', children: [
          { index: true, element: <LazyRoute Component={LazyWorkflowsRoute} /> },
          { path: 'runs/:runId', element: <LazyRoute Component={LazyWorkflowsRoute} /> },
        ]},
      ]},
      { path: 'config', element: <LazyRoute Component={LazySettingsRoute} /> },
    ],
  },
  { path: '*', element: <Navigate to="/projects" replace /> },
]);
