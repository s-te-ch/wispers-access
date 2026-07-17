package dev.wispers.access.android.storage

import android.content.Context
import androidx.room.Database
import androidx.room.Room
import androidx.room.RoomDatabase
import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase
import net.zetetic.database.sqlcipher.SupportOpenHelperFactory

@Database(entities = [ShareEntity::class], version = 4, exportSchema = true)
internal abstract class ShareDatabase : RoomDatabase() {

    abstract fun shareDao(): ShareDao

    companion object {
        // v2: cached site icon. NOT destructive — the table holds the node keys,
        // so wiping it would un-join every share.
        private val MIGRATION_1_2 = object : Migration(1, 2) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE shares ADD COLUMN iconPng BLOB")
                db.execSQL("ALTER TABLE shares ADD COLUMN iconRank INTEGER NOT NULL DEFAULT 0")
            }
        }

        // v3: self-hosted backend URL the invite named (null = managed). Not destructive.
        private val MIGRATION_2_3 = object : Migration(2, 3) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE shares ADD COLUMN backend TEXT")
            }
        }

        // v4: terminal share state (removed/revoked at the hub). Not destructive.
        private val MIGRATION_3_4 = object : Migration(3, 4) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE shares ADD COLUMN terminalState TEXT")
            }
        }

        fun create(context: Context, passphrase: ByteArray): ShareDatabase {
            System.loadLibrary("sqlcipher")
            return Room.databaseBuilder(
                context.applicationContext,
                ShareDatabase::class.java,
                "shares.db",
            )
                .openHelperFactory(SupportOpenHelperFactory(passphrase))
                .addMigrations(MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4)
                .build()
        }
    }
}
