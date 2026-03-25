import express, { Request, Response } from 'express';
import cors from 'cors';
import { config } from './config/env';
import logger from './utils/logger';
import transactionsRouter from './api/routes/transactions.route';
import sep24Router from './api/routes/sep24.route';

const app = express();
const PORT = config.PORT;

app.use(cors());
app.use(express.json());

app.use('/api/transactions', transactionsRouter);

app.get('/health', (req: Request, res: Response) => {
  res.json({ status: 'UP', timestamp: new Date().toISOString() });
});

app.get('/', (req: Request, res: Response) => {
  res.send('AnchorPoint Backend API is running.');
});

// SEP-24 routes
app.use('/sep24', sep24Router);

/* istanbul ignore next */
if (process.env.NODE_ENV !== 'test') {
  app.listen(PORT, () => {
    logger.info(`Backend service listening at http://localhost:${PORT}`);
  });
}

export default app;
