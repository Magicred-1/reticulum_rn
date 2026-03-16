module.exports = {
    preset: 'ts-jest',
    testEnvironment: 'node',
    testMatch: ['**/__tests__/**/*.ts', '**/?(*.)+(spec|test).ts'],
    transform: {
        '^.+\\.ts$': 'ts-jest',
    },
    moduleNameMapper: {
        '^expo-modules-core$': '<rootDir>/node_modules/expo-modules-core',
    },
    setupFilesAfterEnv: ['<rootDir>/jest.setup.ts'],
};
