package dev.wispers.access.android.di

import android.content.Context
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.wispers.access.android.storage.DatabasePassphrase
import dev.wispers.access.android.storage.ShareDao
import dev.wispers.access.android.storage.ShareDatabase
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
internal object StorageModule {

    @Provides
    @Singleton
    fun provideShareDatabase(@ApplicationContext context: Context): ShareDatabase =
        ShareDatabase.create(context, DatabasePassphrase(context).get())

    @Provides
    fun provideShareDao(db: ShareDatabase): ShareDao = db.shareDao()
}
