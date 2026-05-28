package dev.wispers.access.android.di

import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import dev.wispers.access.android.proxy.DefaultHeaderRewriter
import dev.wispers.access.android.proxy.HeaderRewriter
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
object ProxyModule {

    @Provides
    @Singleton
    fun provideHeaderRewriter(): HeaderRewriter = DefaultHeaderRewriter()
}
